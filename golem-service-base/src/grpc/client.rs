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

use crate::tls::material::{TLS_RELOAD_INTERVAL, TlsLoaded, TlsMaterial, TlsSources};
use crate::tls::pem::{PemSource, TlsError};
use arc_swap::ArcSwap;
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
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
    tls: Option<&TlsSnapshot>,
) -> Result<Endpoint, Status> {
    let ip_literal_host = ip_literal_tls_server_name(&uri);
    let endpoint = match tls {
        // `Endpoint::new` gives an `https` URI a default TLS configuration built from its host,
        // which fails for the bracketed host of an IPv6 literal before ours can be set. The
        // configuration set below replaces that default.
        Some(_) => Endpoint::from(uri),
        None => Endpoint::new(uri).map_err(|err| Status::from_error(Box::new(err)))?,
    };
    let mut endpoint = endpoint.connect_timeout(config.connect_timeout);

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

    if let Some(tls) = tls {
        endpoint = endpoint
            .tls_config(tls.tls.tonic_config(&tls.loaded, ip_literal_host))
            .map_err(|err| Status::from_error(Box::new(err)))?;
    }

    Ok(endpoint)
}

/// The TLS material of one client and the server name to check. The material is read again from
/// its files while the client lives, and each new connection is built from what is current then;
/// connections already open keep the certificate they were opened with. A connection built before
/// a reload takes no new calls, because tonic would re-dial it with the old certificate.
pub struct ClientTls {
    material: Arc<TlsMaterial>,
    server_domain_name: Option<String>,
}

impl ClientTls {
    /// `None` when TLS is disabled. A bad setting does not fail here, because the constructors of
    /// the clients are infallible: the error is returned and every connect then fails with it.
    pub fn from_config(
        config: &GrpcClientTlsConfig,
        use_name: &str,
    ) -> Option<Result<Arc<Self>, TlsError>> {
        let GrpcClientTlsConfig::Enabled(tls) = config else {
            return None;
        };
        Some(
            tls.sources()
                .and_then(|sources| TlsMaterial::load(use_name, sources))
                .map(|material| {
                    Arc::new(Self {
                        material,
                        server_domain_name: tls.server_domain_name.clone(),
                    })
                }),
        )
    }

    pub fn material(&self) -> &Arc<TlsMaterial> {
        &self.material
    }

    /// The tonic configuration for a new connection, built from `loaded`. Without a configured
    /// domain name, `ip_literal_server_name` is the name to check.
    pub fn tonic_config(
        &self,
        loaded: &TlsLoaded,
        ip_literal_server_name: Option<String>,
    ) -> ClientTlsConfig {
        use tonic::transport::{Certificate, Identity};

        let mut config = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(&loaded.ca_pem))
            .identity(Identity::from_pem(&loaded.cert_pem, &loaded.key_pem));
        if let Some(name) = self.server_domain_name.clone().or(ip_literal_server_name) {
            config = config.domain_name(name);
        }
        config
    }
}

/// The TLS one connection is built with: the client's settings and the material that was current
/// when the connection was made. The connection keeps it, so that a reload can be told apart.
#[derive(Clone)]
struct TlsSnapshot {
    tls: Arc<ClientTls>,
    loaded: Arc<TlsLoaded>,
}

impl TlsSnapshot {
    /// Reads the current material once, so the connection is built from the very material it is
    /// later compared by.
    fn of(tls: Arc<ClientTls>) -> Self {
        let loaded = tls.material.current();
        Self { tls, loaded }
    }

    /// Whether no reload has replaced the material since. Asked on every call, so it takes no
    /// lock.
    fn is_current(&self) -> bool {
        self.tls.material.is_current(&self.loaded)
    }
}

/// The TLS of a client family (a client and all its clones), shared by them. When the settings
/// could not be loaded, the error is kept and the load is tried again on a later connect, at most
/// once per `TLS_RELOAD_INTERVAL`, so a file that appears after the start is picked up.
struct SharedClientTls {
    use_name: String,
    config: GrpcClientTlsConfig,
    state: ArcSwap<Result<Arc<ClientTls>, TlsError>>,
    /// When the load was last attempted after the start; taken only while the state is an error.
    last_attempt: std::sync::Mutex<Option<Instant>>,
}

/// Absent when TLS is off.
type ClientTlsState = Option<Arc<SharedClientTls>>;

fn client_tls_state(config: &GrpcClientTlsConfig, use_name: &str) -> ClientTlsState {
    let loaded = ClientTls::from_config(config, use_name)?;
    if let Err(err) = &loaded {
        warn!(
            "TLS settings of {use_name} cannot be loaded; connections fail, and the load is tried again every {} s: {err}",
            TLS_RELOAD_INTERVAL.as_secs()
        );
    }
    Some(Arc::new(SharedClientTls {
        use_name: use_name.to_string(),
        config: config.clone(),
        state: ArcSwap::from_pointee(loaded),
        last_attempt: std::sync::Mutex::new(Some(Instant::now())),
    }))
}

impl SharedClientTls {
    /// The TLS to connect with, or the reason there is none.
    fn current(&self) -> Result<Arc<ClientTls>, Status> {
        if let Ok(tls) = &**self.state.load() {
            return Ok(tls.clone());
        }
        let mut last_attempt = self
            .last_attempt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Another connect may have loaded it while this one waited for the lock.
        if let Ok(tls) = &**self.state.load() {
            return Ok(tls.clone());
        }
        if last_attempt.is_none_or(|at| at.elapsed() >= TLS_RELOAD_INTERVAL) {
            *last_attempt = Some(Instant::now());
            if let Some(loaded) = ClientTls::from_config(&self.config, &self.use_name) {
                match &loaded {
                    Ok(_) => debug!(
                        "TLS settings of {} loaded after an earlier failure",
                        self.use_name
                    ),
                    Err(err) => warn!(
                        "TLS settings of {} still cannot be loaded: {err}",
                        self.use_name
                    ),
                }
                self.state.store(Arc::new(loaded));
            }
        }
        match &**self.state.load() {
            Ok(tls) => Ok(tls.clone()),
            Err(err) => Err(Status::failed_precondition(format!(
                "TLS configuration: {err}"
            ))),
        }
    }

    fn material(&self) -> Option<Arc<TlsMaterial>> {
        match &**self.state.load() {
            Ok(tls) => Some(tls.material.clone()),
            Err(_) => None,
        }
    }

    /// Lets the next connect try the load again at once.
    fn retry_now(&self) {
        *self
            .last_attempt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

/// The TLS server name for a URI whose host is an IPv6 literal. Without a configured domain name,
/// tonic takes the server name from the URI host, which keeps the brackets of an IPv6 literal, and
/// rustls rejects a bracketed name.
fn ip_literal_tls_server_name(uri: &Uri) -> Option<String> {
    uri.host()?
        .strip_prefix('[')?
        .strip_suffix(']')
        .map(str::to_string)
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
/// A client with one fixed target uses `Duration::MAX` instead. Its pool slots
/// rotate by connection age rather than by idle time.
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
/// connection stops it being handed out again. Only a connection whose transport
/// failed is also gone, and being gone is what releases the requests already
/// riding it: a status the peer sent back is proof the transport works, however
/// unwelcome the answer, and cutting those requests short would cost every one
/// of them a retry.
///
/// A guard that reads only the second question reads nothing at all on the
/// commoner path, which is how a connection one caller had just evicted came to
/// be handed straight to the next.
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
    /// Marked before `forget` takes the lock over the cached entry, so a caller
    /// reading that entry under the lock finds the connection already retired
    /// rather than reusing one its owner has given up on. The other order leaves
    /// the reader seeing it live.
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
    established: Instant,
    max_age: Duration,
    /// What the connection's TLS was built from. `None` without TLS.
    tls: Option<TlsSnapshot>,
}

impl<T: Clone> GrpcClientConnection<T> {
    /// Whether a new call may be given this connection: it is not retired, not past its age, and
    /// its TLS material is still the client's current one.
    ///
    /// tonic re-dials a lost connection inside the `Channel`, with the TLS configuration the
    /// `Channel` was built with, so a `Channel` built before a reload would present the old
    /// certificate on its next handshake, and a multi-target client never replaces one for its
    /// age. A connection whose material a reload has replaced is therefore treated as one past
    /// its age: the next call builds a new one from the current material, and the calls and
    /// streams already riding it finish there.
    fn reusable(&self) -> bool {
        !self.retirement.is_retired()
            && (self.max_age.is_zero() || self.established.elapsed() < self.max_age)
            && self.tls.as_ref().is_none_or(TlsSnapshot::is_current)
    }
}

fn connect_shared<T>(
    endpoint: Endpoint,
    tls: Option<TlsSnapshot>,
    config: &GrpcClientConfig,
    build_client: ClientFactory<T>,
    max_connection_age: Duration,
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
                established: Instant::now(),
                // Rotate on use, independently per slot and caller process.
                // Never extend the configured maximum age.
                max_age: max_connection_age.mul_f64(rand::random_range(0.8..=1.0)),
                tls,
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
    max_connection_age: Duration,
    /// See [`ClientTls`]. Shared by every clone.
    tls: ClientTlsState,
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
            max_connection_age: Duration::ZERO,
            tls: None,
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

            if connected.reusable() {
                return Ok(connected);
            }

            // A caller that joined an attempt already under way can be handed a
            // connection that has since expired, been retired, or had its TLS
            // material replaced by a reload. Recheck at handoff, so a waiter
            // delayed past its lifetime gets a replacement.
            attempts_left = attempts_left.saturating_sub(1);
            if attempts_left == 0 {
                return Err(Status::unavailable(
                    "connection expired or retired before it could be used",
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
                Some(Ok(connected)) if connected.reusable() => {
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
        let (endpoint, tls) = match &self.tls {
            Some(shared) => {
                let tls = shared.current()?;
                tls.material.start_reloader(TLS_RELOAD_INTERVAL);
                let tls = TlsSnapshot::of(tls);
                (
                    build_endpoint(target.clone(), config, Some(&tls))?,
                    Some(tls),
                )
            }
            None => (build_endpoint(target.clone(), config, None)?, None),
        };

        let attempt = match self.by_target.entry_async(target.clone()).await {
            Entry::Occupied(mut entry) => match entry.get().peek() {
                // Already connected. This is the reuse path.
                Some(Ok(connected)) if connected.reusable() => {
                    connected.last_used.touch();
                    return Ok(connected.clone());
                }
                None => entry.get().clone(),
                // A failed attempt describes a connection nobody is still making
                // and a retired one describes a connection its owner has given
                // up on, so the caller gets its own rather than inheriting
                // either verdict.
                Some(_) => {
                    let attempt = connect_shared(
                        endpoint,
                        tls,
                        config,
                        build_client.clone(),
                        self.max_connection_age,
                    );
                    *entry.get_mut() = attempt.clone();
                    self.drive(target.clone(), attempt.clone());
                    attempt
                }
            },
            Entry::Vacant(entry) => {
                let attempt = connect_shared(
                    endpoint,
                    tls,
                    config,
                    build_client.clone(),
                    self.max_connection_age,
                );
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

/// A connection pool for one fixed service address.
///
/// Each slot uses the same connection establishment and failure handling as
/// [`MultiTargetGrpcClient`]. Clones share the pool and round-robin position.
#[derive(Clone)]
pub struct GrpcClient<T: Clone> {
    endpoint: Uri,
    pool: Arc<[MultiTargetGrpcClient<T>]>,
    next: Arc<AtomicUsize>,
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
        let client_factory = Arc::new(client_factory);
        // One for the whole pool, so there is one poller and one log line per renewal.
        let tls = client_tls_state(&config.tls, &format!("{} client", target_name.as_ref()));
        let pool = (0..config.connection_pool_size.get())
            .map(|_| {
                let factory = client_factory.clone();
                let mut client = MultiTargetGrpcClient::with_sweep_and_tls(
                    target_name.as_ref(),
                    move |channel, size| factory(channel, size),
                    config.clone(),
                    Duration::MAX,
                    PRUNE_INTERVAL,
                    tls.clone(),
                );
                client.connections.max_connection_age = config.max_connection_age;
                client
            })
            .collect();
        Self {
            endpoint,
            pool,
            next: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The TLS material shared by the pool, if TLS is enabled and its settings loaded.
    #[doc(hidden)]
    pub fn tls_material(&self) -> Option<Arc<TlsMaterial>> {
        self.pool[0].tls_material()
    }

    /// Lets the next connect try loading the TLS settings again at once.
    #[doc(hidden)]
    pub fn retry_tls_load_now(&self) {
        self.pool[0].retry_tls_load_now()
    }

    fn select(&self) -> &MultiTargetGrpcClient<T> {
        &self.pool[self.next.fetch_add(1, Ordering::Relaxed) % self.pool.len()]
    }

    pub async fn call<F, R>(&self, description: impl AsRef<str>, f: F) -> Result<R, Status>
    where
        F: for<'a> Fn(&'a mut T) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
            + Send,
    {
        self.select()
            .call(description, self.endpoint.clone(), f)
            .await
    }

    /// Performs one attempt at the call itself, even for `Unavailable`, for
    /// operations that are unsafe to retry without an idempotency key.
    pub async fn call_without_retry<F, R>(
        &self,
        description: impl AsRef<str>,
        f: F,
    ) -> Result<R, Status>
    where
        F: for<'a> Fn(&'a mut T) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
            + Send,
    {
        self.select()
            .call_without_retry(description, self.endpoint.clone(), f)
            .await
    }

    /// [`Self::call`] or [`Self::call_without_retry`], chosen by `retry_the_call`.
    pub(crate) async fn call_with_retry<F, R>(
        &self,
        description: impl AsRef<str>,
        retry_the_call: bool,
        f: F,
    ) -> Result<R, Status>
    where
        F: for<'a> Fn(&'a mut T) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
            + Send,
    {
        self.select()
            .call_with_retry(description, self.endpoint.clone(), retry_the_call, f)
            .await
    }

    /// See [`MultiTargetGrpcClient::with_redirect`]. Applies to every connection of the pool.
    pub fn with_redirect(self, is_redirect: fn(&Status) -> bool) -> Self {
        Self {
            endpoint: self.endpoint,
            pool: self
                .pool
                .iter()
                .cloned()
                .map(|client| client.with_redirect(is_redirect))
                .collect(),
            next: self.next,
        }
    }
}

#[derive(Clone)]
pub struct MultiTargetGrpcClient<T: Clone> {
    config: GrpcClientConfig,
    connections: Connections<T>,
    client_factory: ClientFactory<T>,
    target_name: String,
    /// Recognises an answer that sends the caller to another target. See [`Self::with_redirect`].
    redirect: Option<fn(&Status) -> bool>,
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
        let tls = client_tls_state(&config.tls, &format!("{} client", target_name.as_ref()));
        Self::with_sweep_and_tls(
            target_name,
            client_factory,
            config,
            idle_ttl,
            prune_interval,
            tls,
        )
    }

    fn with_sweep_and_tls(
        target_name: impl AsRef<str>,
        client_factory: impl Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync + 'static,
        config: GrpcClientConfig,
        idle_ttl: Duration,
        prune_interval: Duration,
        tls: ClientTlsState,
    ) -> Self {
        let mut connections = Connections::new(idle_ttl, prune_interval);
        connections.tls = tls;
        Self {
            config,
            connections,
            client_factory: Arc::new(client_factory),
            target_name: target_name.as_ref().to_string(),
            redirect: None,
        }
    }

    /// Returns a status that `is_redirect` accepts to the caller on its first attempt: no retry,
    /// and the connection it came over stays cached.
    ///
    /// A redirect is an answer, not a failure. The peer is healthy and has said where to go
    /// instead, so asking it again gets the same answer, and replacing the connection would
    /// discard one that works.
    pub fn with_redirect(mut self, is_redirect: fn(&Status) -> bool) -> Self {
        self.redirect = Some(is_redirect);
        self
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
        self.call_with_retry(description, endpoint, true, f).await
    }

    /// Performs one attempt at the call itself, even for `Unavailable`, for
    /// operations whose request stream cannot be replayed.
    ///
    /// A connect that fails is still retried. `f` has not run at that point, so
    /// nothing of the request has been consumed and the next attempt is the
    /// first one the peer could ever see.
    pub async fn call_without_retry<F, R>(
        &self,
        description: impl AsRef<str>,
        endpoint: Uri,
        f: F,
    ) -> Result<R, Status>
    where
        F: for<'a> Fn(&'a mut T) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
            + Send,
    {
        self.call_with_retry(description, endpoint, false, f).await
    }

    pub(crate) async fn call_with_retry<F, R>(
        &self,
        description: impl AsRef<str>,
        endpoint: Uri,
        retry_the_call: bool,
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
                Err(e) if self.redirect.is_some_and(|is_redirect| is_redirect(&e)) => {
                    attempts.redirected(&e);
                    break Err(e);
                }
                Err(e) if requires_reconnect(&e) => {
                    self.retire(&endpoint, &entry, &e).await;
                    if !retry_the_call {
                        attempts.not_replayable(&e);
                        break Err(e);
                    }
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

    /// The TLS material of this client, if TLS is enabled and its settings loaded.
    #[doc(hidden)]
    pub fn tls_material(&self) -> Option<Arc<TlsMaterial>> {
        self.connections.tls.as_ref()?.material()
    }

    /// Lets the next connect try loading the TLS settings again at once.
    #[doc(hidden)]
    pub fn retry_tls_load_now(&self) {
        if let Some(tls) = &self.connections.tls {
            tls.retry_now();
        }
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
    /// Reusable connections to a fixed service address, selected round-robin
    /// per call. Expired connections can remain alive while existing calls or
    /// streams use them. Multi-target clients keep one connection per explicit
    /// target and ignore this setting.
    pub connection_pool_size: NonZeroUsize,
    /// Maximum age before a fixed-target pool slot reconnects on its next use.
    /// Zero disables rotation. Each connection uses 80–100% of this age to
    /// stagger replacements. Existing calls and streams are not interrupted.
    /// Multi-target clients ignore this setting.
    #[serde(with = "humantime_serde")]
    pub max_connection_age: Duration,
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
            connection_pool_size: NonZeroUsize::new(4).unwrap(),
            max_connection_age: Duration::from_secs(60),
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
        let _ = writeln!(
            &mut result,
            "connection_pool_size: {}",
            self.connection_pool_size
        );
        let _ = writeln!(
            &mut result,
            "max_connection_age: {:?}",
            self.max_connection_age
        );
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

/// Each item comes from a PEM file or an inline value (the PEM, Base64 on one line), not both.
/// Files are polled and a renewed certificate is used for new connections; an inline value
/// changes only with a restart.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnabledGrpcClientTlsConfig {
    /// client-specific certificate  — issued by cluster CA
    #[serde(default)]
    pub client_cert: Option<Base64>,
    #[serde(default)]
    pub client_cert_file: Option<PathBuf>,
    /// private key for client_cert
    #[serde(default)]
    pub client_key: Option<Base64>,
    #[serde(default)]
    pub client_key_file: Option<PathBuf>,
    /// CA certificate used to validate server certificates (PEM)
    #[serde(default)]
    pub server_ca_cert: Option<Base64>,
    #[serde(default)]
    pub server_ca_cert_file: Option<PathBuf>,
    /// The name the server's certificate must carry. Without it, the certificate is checked
    /// against the host of the address the client connects to (an IPv6 literal without its
    /// brackets).
    #[serde(default)]
    pub server_domain_name: Option<String>,
}

impl SafeDisplay for EnabledGrpcClientTlsConfig {
    fn to_safe_string(&self) -> String {
        use sha2::{Digest, Sha256};

        fn item(
            result: &mut String,
            name: &str,
            value: Option<&Base64>,
            file: Option<&Path>,
            show_fingerprint: bool,
        ) {
            if let Some(file) = file {
                let _ = writeln!(result, "{name}_file: {}", file.display());
            } else if let (true, Some(value)) = (show_fingerprint, value) {
                let _ = writeln!(
                    result,
                    "{name}_sha256: {}",
                    hex::encode(Sha256::digest(&value.0))
                );
            } else if value.is_some() {
                let _ = writeln!(result, "{name}: *******");
            } else {
                let _ = writeln!(result, "{name}: <not set>");
            }
        }

        let mut result = String::new();
        item(
            &mut result,
            "client_cert",
            self.client_cert.as_ref(),
            self.client_cert_file.as_deref(),
            true,
        );
        item(
            &mut result,
            "client_key",
            self.client_key.as_ref(),
            self.client_key_file.as_deref(),
            false,
        );
        item(
            &mut result,
            "server_ca_cert",
            self.server_ca_cert.as_ref(),
            self.server_ca_cert_file.as_deref(),
            true,
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
    pub fn sources(&self) -> Result<TlsSources, TlsError> {
        Ok(TlsSources {
            cert: PemSource::from_config(
                "client_cert",
                self.client_cert.as_ref(),
                self.client_cert_file.as_deref(),
            )?,
            key: PemSource::from_config(
                "client_key",
                self.client_key.as_ref(),
                self.client_key_file.as_deref(),
            )?,
            ca: PemSource::from_config(
                "server_ca_cert",
                self.server_ca_cert.as_ref(),
                self.server_ca_cert_file.as_deref(),
            )?,
        })
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

/// The retry bookkeeping the `call` loop uses: decide whether there is another
/// attempt to make, and record the outcome either way.
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

    /// Records a failure on a call whose request cannot be replayed, so there is
    /// no second attempt to make regardless of what the failure was. Distinct
    /// from [`Self::gave_up`], which is for a failure retrying could not help.
    fn not_replayable(&self, e: &Status) {
        self.span
            .in_scope(|| warn!("gRPC call failed: {:?}, request is not replayable", e));
        record_internal_grpc_failure(self.target_name, self.description, e);
    }

    /// Records an answer that sends the caller elsewhere: neither a success nor a failure of this
    /// target.
    fn redirected(&self, e: &Status) {
        self.span
            .in_scope(|| debug!(answer = e.message(), "gRPC call redirected"));
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

/// Whether a status is an `UNAVAILABLE` with no transport failure under it, as a service gives
/// that refuses work for now.
///
/// This does not tell a peer's answer from the client's own failures: a connect that fails or
/// times out, a connection that expired or was retired before it could be used, and one that was
/// retired while the request waited on it are all built here as a plain `Status::unavailable` with
/// no source and pass as well. Only a caller that knows the call itself ran, as the shard manager
/// router does by watching the call's own result, can take it for an answer.
pub(crate) fn peer_answered_unavailable(e: &Status) -> bool {
    e.code() == Code::Unavailable && !transport_failed(e)
}

pub(crate) fn requires_reconnect(e: &Status) -> bool {
    // A request that ran out of its own time says nothing about the health of
    // the connection it ran on. Without this the registry client
    // (request_timeout = 30s) would tear down the channel every caller shares
    // each time one request ran long.
    if request_timed_out(e) {
        return false;
    }

    worth_reconnecting(e.code(), transport_failed(e))
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
/// stream and leaves the connection carrying everybody else. [`transport_failed`]
/// now keeps a reset out of the transport half. A `REFUSED_STREAM` still
/// reconnects, because tonic maps it to `Unavailable` and that half asks no
/// further: one reconnect over a stream the peer would not take is the cheaper
/// way to be wrong, and it no longer releases the requests beside it.
fn worth_reconnecting(code: Code, transport_failed: bool) -> bool {
    code == Code::Unavailable || connection_gone(code, transport_failed)
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

/// [`connection_gone`], asked of a status.
///
/// Only ever asked about one [`requires_reconnect`] has already accepted, so a
/// request that ran out of its own time cannot reach here: it is excluded there,
/// once.
fn connection_is_gone(e: &Status) -> bool {
    connection_gone(e.code(), transport_failed(e))
}

/// Whether the connection carrying a request is gone, as opposed to one request
/// on it having gone wrong.
///
/// A stricter question than [`worth_reconnecting`], deliberately. Dropping a
/// connection from the cache costs one reconnect, so it is worth doing on a
/// hint; declaring it gone releases every other request riding it, so it has to
/// be right.
///
/// Both halves are needed. A peer that answered a status did so over a
/// transport that works, whatever the code, so the transport has to have failed.
/// And not every transport failure is the connection's: the code separates the
/// ones that are. Measured against a real peer, a connect into a blackhole and
/// an expired keep-alive ping arrive as `Unavailable`, a connection the kernel
/// gives up on arrives as `Unknown`, and a connection that closes with requests
/// still on it arrives as `Cancelled` — the commonest of the three, because it
/// is what killing a busy pod looks like. `Internal`, `ResourceExhausted` and
/// `PermissionDenied` are left out: tonic only derives those from an HTTP/2
/// reset, which ends one stream and leaves the connection carrying everyone
/// else.
///
/// The code alone cannot finish the job, because two reset reasons land on codes
/// in the first set: tonic maps `CANCEL` to `Cancelled` and `REFUSED_STREAM` to
/// `Unavailable`. Read here, either declared a healthy connection gone and
/// released every request beside the reset one with `Unavailable`; a call that
/// could not be replayed was failed for good over a stream it was never on.
/// That is why [`transport_failed`] answers `false` for any reset before the
/// code is consulted.
fn connection_gone(code: Code, transport_failed: bool) -> bool {
    matches!(code, Code::Unavailable | Code::Unknown | Code::Cancelled) && transport_failed
}

/// Whether the transport under a status failed, as opposed to the peer having
/// answered over one that works, or having reset the one stream the request was
/// on.
///
/// `Channel` wraps everything that goes wrong below it in a
/// `tonic::transport::Error`, an HTTP/2 reset of one stream included, so the
/// wrapper alone says only that the failure came from below tonic. The
/// `h2::Error` further down the chain knows whether it came from RST_STREAM,
/// and a reset ends one stream while the connection carries everybody else. A
/// connection that dies reaches h2 as an I/O error or a GOAWAY instead, and
/// neither is a reset.
fn transport_failed(e: &Status) -> bool {
    has_transport_source(e) && !reset_one_stream(e)
}

fn has_transport_source(e: &Status) -> bool {
    std::error::Error::source(e)
        .map(|source| source.is::<tonic::transport::Error>())
        .unwrap_or(false)
}

/// Whether the failure was an HTTP/2 RST_STREAM, which ends the one stream it
/// names and nothing else.
///
/// Found by walking the source chain: `Status::from_error` keeps the whole
/// `tonic::transport::Error` it was given as the source, and the `hyper::Error`
/// under that carries the `h2::Error` the reset arrived as.
fn reset_one_stream(e: &Status) -> bool {
    let mut source = std::error::Error::source(e);
    while let Some(err) = source {
        if let Some(h2) = err.downcast_ref::<h2::Error>() {
            return h2.is_reset();
        }
        source = err.source();
    }
    false
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::StreamExt;
    use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
    use golem_api_grpc::proto::golem::workerexecutor::v1::{
        AssignShardsRequest, ConnectWorkerRequest,
    };
    use http_body_util::BodyExt;
    use test_r::test;

    type ProbeClient = WorkerExecutorClient<OtelGrpcService<Channel>>;

    #[test]
    fn pool_configuration_accepts_single_connection_and_rejects_zero_connections() {
        let defaults = GrpcClientConfig::default();
        assert_eq!(defaults.connection_pool_size.get(), 4);
        assert_eq!(defaults.max_connection_age, Duration::from_secs(60));
        let mut config = serde_json::to_value(defaults).unwrap();
        config["connection_pool_size"] = 1.into();
        config["max_connection_age"] = "0s".into();
        let single: GrpcClientConfig = serde_json::from_value(config.clone()).unwrap();
        assert_eq!(single.connection_pool_size.get(), 1);
        assert_eq!(single.max_connection_age, Duration::ZERO);
        config["connection_pool_size"] = 0.into();
        assert!(serde_json::from_value::<GrpcClientConfig>(config).is_err());
    }

    struct PoolPeer {
        target: Uri,
        task: tokio::task::JoinHandle<()>,
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        closed: Arc<tokio::sync::Notify>,
    }

    impl Drop for PoolPeer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    // A real HTTP/2 peer identifies the TCP connection in each gRPC response.
    // The hold header lets a call remain in flight while its slot rotates.
    async fn pool_peer() -> PoolPeer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let closed = Arc::new(tokio::sync::Notify::new());
        let closed_peer = closed.clone();
        let (started_peer, release_peer) = (started.clone(), release.clone());
        let peer = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, remote) = accepted.unwrap();
                        let (started, release) = (started_peer.clone(), release_peer.clone());
                        let closed = closed_peer.clone();
                        connections.spawn(async move {
                            let service = hyper::service::service_fn(move |request: http::Request<hyper::body::Incoming>| {
                                let (started, release) = (started.clone(), release.clone());
                                async move {
                                    let streaming = request.uri().path().ends_with("/ConnectWorker");
                                    if request.headers().contains_key("hold") {
                                        started.notify_one();
                                        release.notified().await;
                                    }
                                    let body = if streaming {
                                        let frames = futures::stream::once(async move {
                                            release.notified().await;
                                            Ok::<_, std::convert::Infallible>(hyper::body::Frame::data(bytes::Bytes::from_static(&[0, 0, 0, 0, 0])))
                                        }).chain(futures::stream::once(async {
                                            let mut trailers = http::HeaderMap::new();
                                            trailers.insert("grpc-status", "0".parse().unwrap());
                                            Ok(hyper::body::Frame::trailers(trailers))
                                        }));
                                        http_body_util::StreamBody::new(frames).boxed_unsync()
                                    } else {
                                        http_body_util::Full::new(bytes::Bytes::from_static(&[0, 0, 0, 0, 0])).boxed_unsync()
                                    };
                                    let mut response = http::Response::builder()
                                        .header("content-type", "application/grpc")
                                        .header("connection-id", remote.port().to_string());
                                    if !streaming {
                                        response = response.header("grpc-status", "0");
                                    }
                                    Ok::<_, std::convert::Infallible>(response
                                        .body(body)
                                        .unwrap())
                                }
                            });
                            let _ = hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
                                .serve_connection(hyper_util::rt::TokioIo::new(socket), service).await;
                            closed.notify_one();
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        PoolPeer {
            target,
            task: peer,
            started,
            release,
            closed,
        }
    }

    async fn probe(client: &mut ProbeClient, hold: bool) -> Result<String, Status> {
        let mut request = tonic::Request::new(AssignShardsRequest::default());
        if hold {
            request
                .metadata_mut()
                .insert("hold", "true".parse().unwrap());
        }
        let response = client.assign_shards(request).await?;
        Ok(response
            .metadata()
            .get("connection-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned())
    }

    #[test]
    #[test_r::timeout("15s")]
    async fn fixed_target_pool_shares_round_robin_connections_across_clones() {
        let peer = pool_peer().await;
        let client = GrpcClient::new(
            "test",
            |channel, _| ProbeClient::new(channel),
            peer.target.clone(),
            GrpcClientConfig {
                connection_pool_size: NonZeroUsize::new(3).unwrap(),
                max_connection_age: Duration::ZERO,
                ..one_attempt()
            },
        );
        let clone = client.clone();
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(
                client
                    .call("probe", |c| Box::pin(probe(c, false)))
                    .await
                    .unwrap(),
            );
            ids.push(
                clone
                    .call_without_retry("probe", |c| Box::pin(probe(c, false)))
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            3
        );
        assert_eq!(&ids[..3], &ids[3..]);
    }

    #[test]
    #[test_r::timeout("15s")]
    async fn fixed_target_rotation_preserves_in_flight_calls_and_shares_replacement() {
        let peer = pool_peer().await;
        let client = GrpcClient::new(
            "test",
            |channel, _| ProbeClient::new(channel),
            peer.target.clone(),
            GrpcClientConfig {
                connection_pool_size: NonZeroUsize::MIN,
                max_connection_age: Duration::from_millis(200),
                ..one_attempt()
            },
        );
        let first = client
            .call("probe", |c| Box::pin(probe(c, false)))
            .await
            .unwrap();
        let held_client = client.clone();
        let held = tokio::spawn(async move {
            held_client
                .call_without_retry("held", |c| Box::pin(probe(c, true)))
                .await
        });
        peer.started.notified().await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        let calls = (0..8).map(|_| client.call("probe", |c| Box::pin(probe(c, false))));
        let replacements = futures::future::try_join_all(calls).await.unwrap();
        assert!(replacements.iter().all(|id| id == &replacements[0]));
        assert_ne!(replacements[0], first);
        peer.release.notify_one();
        assert_eq!(held.await.unwrap().unwrap(), first);
    }

    #[test]
    #[test_r::timeout("15s")]
    async fn fixed_target_rotation_preserves_returned_stream_and_releases_old_connection() {
        let peer = pool_peer().await;
        let client = GrpcClient::new(
            "test",
            |channel, _| ProbeClient::new(channel),
            peer.target.clone(),
            GrpcClientConfig {
                connection_pool_size: NonZeroUsize::MIN,
                max_connection_age: Duration::from_millis(200),
                ..one_attempt()
            },
        );
        let response = client
            .call_without_retry("stream", |c| {
                Box::pin(c.connect_worker(ConnectWorkerRequest::default()))
            })
            .await
            .unwrap();
        let first = response
            .metadata()
            .get("connection-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let mut stream = response.into_inner();
        tokio::time::sleep(Duration::from_millis(250)).await;
        let replacement = client
            .call("probe", |c| Box::pin(probe(c, false)))
            .await
            .unwrap();
        assert_ne!(replacement, first);
        peer.release.notify_one();
        assert_eq!(stream.message().await.unwrap(), Some(Default::default()));
        assert_eq!(stream.message().await.unwrap(), None);
        let trailers = stream.trailers().await.unwrap().unwrap();
        assert_eq!(trailers.get("grpc-status").unwrap(), "0");
        drop(stream);
        peer.closed.notified().await;
    }

    #[test]
    #[test_r::timeout("15s")]
    async fn fixed_target_waiter_rechecks_age_at_handoff() {
        let peer = pool_peer().await;
        let client = GrpcClient::new(
            "test",
            |channel, _| ProbeClient::new(channel),
            peer.target.clone(),
            GrpcClientConfig {
                connection_pool_size: NonZeroUsize::MIN,
                max_connection_age: Duration::from_millis(200),
                ..one_attempt()
            },
        );
        let first = client
            .call("probe", |c| Box::pin(probe(c, false)))
            .await
            .unwrap();
        let handoffs = Arc::new(AtomicUsize::new(0));
        client.pool[0].connections.set_interleave(Arc::new(move || {
            let first_handoff = handoffs.fetch_add(1, Ordering::Relaxed) == 0;
            async move {
                if first_handoff {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
            .boxed()
        }));
        let replacement = client
            .call("probe", |c| Box::pin(probe(c, false)))
            .await
            .unwrap();
        assert_ne!(replacement, first);
    }

    #[test]
    #[test_r::timeout("15s")]
    async fn single_connection_without_rotation_and_explicit_targets_keep_reusing_connections() {
        let peer = pool_peer().await;
        let single = GrpcClient::new(
            "test",
            |channel, _| ProbeClient::new(channel),
            peer.target.clone(),
            GrpcClientConfig {
                connection_pool_size: NonZeroUsize::MIN,
                max_connection_age: Duration::ZERO,
                ..one_attempt()
            },
        );
        let routed = MultiTargetGrpcClient::new(
            "test",
            |channel, _| ProbeClient::new(channel),
            GrpcClientConfig {
                connection_pool_size: NonZeroUsize::new(3).unwrap(),
                max_connection_age: Duration::from_millis(1),
                ..one_attempt()
            },
        );
        let first = single
            .call("probe", |c| Box::pin(probe(c, false)))
            .await
            .unwrap();
        let routed_first = routed
            .call("probe", peer.target.clone(), |c| Box::pin(probe(c, false)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        for _ in 0..6 {
            assert_eq!(
                single
                    .call("probe", |c| Box::pin(probe(c, false)))
                    .await
                    .unwrap(),
                first
            );
            assert_eq!(
                routed
                    .call("probe", peer.target.clone(), |c| Box::pin(probe(c, false)))
                    .await
                    .unwrap(),
                routed_first
            );
        }
    }

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
            None,
            &GrpcClientConfig {
                connect_timeout,
                ..GrpcClientConfig::default()
            },
            Arc::new(|_, _| ()),
            Duration::ZERO,
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
    /// Two attempts in total with no waiting between them, so a test can count
    /// them without paying the default backoff.
    fn retries_quickly() -> GrpcClientConfig {
        GrpcClientConfig {
            retries_on_unavailable: RetryConfig {
                max_attempts: 2,
                min_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(1),
                multiplier: 1.0,
                max_jitter_factor: None,
            },
            ..GrpcClientConfig::default()
        }
    }

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
    /// connection established.
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
            None,
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
            3,
            "the caller did not establish exactly three connections before \
             giving up; written out rather than read from CONNECT_ATTEMPTS, \
             which would agree with whatever that came to say"
        );
    }

    /// Fixed-target pool slots share the multi-target call path, but remain
    /// exempt from idle sweeping.
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
            client.pool[0].connections.idle_ttl,
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
        let endpoint = build_endpoint(target.clone(), &config, None).unwrap();
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
    /// transport error. `transport_failed` reads the reset out of the chain
    /// before the code is consulted, but the codes are pinned as well, against
    /// the ones tonic only ever derives from an HTTP/2 reset: reading one of
    /// those as a dead connection would end every request riding a connection
    /// that is still perfectly good. The two reset reasons whose codes collide
    /// with a dead connection's, `CANCEL` and `REFUSED_STREAM`, cannot be built
    /// without a peer, so they are covered by the integration tests.
    #[test]
    async fn only_some_codes_describe_a_connection_that_is_gone() {
        // Measured against a real peer: a blackholed connect and an expired
        // keep-alive ping arrive as Unavailable, a connection the kernel gives up
        // on arrives as Unknown, and a connection closing with requests still on
        // it arrives as Cancelled.
        for code in [Code::Unavailable, Code::Unknown, Code::Cancelled] {
            assert!(
                connection_gone(code, true),
                "{code:?} over a failed transport is what a dead connection \
                 arrives as"
            );
            assert!(
                !connection_gone(code, false),
                "{code:?} with nothing wrong underneath is an answer, not a \
                 dead connection"
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
                !connection_gone(code, true),
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
                    !connection_gone(code, transport_failed)
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
                established: Instant::now(),
                max_age: Duration::ZERO,
                tls: None,
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
        let endpoint = build_endpoint(target.clone(), &config, None).unwrap();
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
        let endpoint = build_endpoint(target.clone(), &config, None).unwrap();
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
        let endpoint =
            build_endpoint("http://127.0.0.1:1/".parse().unwrap(), &config, None).unwrap();
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

    /// A call whose request cannot be replayed is attempted exactly once, even
    /// though `Unavailable` is the failure everything else retries.
    ///
    /// The peer here accepts the connection, so `f` runs and its failure is a
    /// call failure rather than a connect failure. A connect that fails is still
    /// retried, and has to be: `f` never ran, so nothing of the request was
    /// consumed.
    #[test]
    async fn a_call_that_cannot_be_replayed_is_attempted_once() {
        let (target, _peer) = silent_peer().await;
        let client = MultiTargetGrpcClient::new("test", |_, _| (), retries_quickly());
        let attempts = Arc::new(AtomicU64::new(0));
        let counted = attempts.clone();

        let result = client
            .call_without_retry("non-replayable", target, move |_: &mut ()| {
                counted.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err::<(), Status>(Status::unavailable("failed")) })
            })
            .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    /// The other half, so the test above is measuring the gate rather than a
    /// client that would not have retried anyway.
    #[test]
    async fn the_same_failure_is_retried_when_the_request_can_be_replayed() {
        let (target, _peer) = silent_peer().await;
        let client = MultiTargetGrpcClient::new("test", |_, _| (), retries_quickly());
        let attempts = Arc::new(AtomicU64::new(0));
        let counted = attempts.clone();

        let result = client
            .call("replayable", target, move |_: &mut ()| {
                counted.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err::<(), Status>(Status::unavailable("failed")) })
            })
            .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn build_endpoint_uses_the_ip_literal_server_name_when_none_is_configured() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert = certified.cert.pem().into_bytes();
        let inline = |setting: &str, bytes: Vec<u8>| PemSource::Inline {
            setting: setting.to_string(),
            bytes,
        };
        let material = TlsMaterial::load(
            "test client",
            TlsSources {
                cert: inline("client_cert", cert.clone()),
                key: inline(
                    "client_key",
                    certified.signing_key.serialize_pem().into_bytes(),
                ),
                ca: inline("server_ca_cert", cert),
            },
        )
        .unwrap();
        let tls = TlsSnapshot::of(Arc::new(ClientTls {
            material,
            server_domain_name: None,
        }));
        let uri: Uri = "https://[::1]:1".parse().unwrap();
        assert_eq!(ip_literal_tls_server_name(&uri).as_deref(), Some("::1"));
        build_endpoint(uri, &GrpcClientConfig::default(), Some(&tls)).unwrap();
    }

    #[test]
    fn ip_literal_tls_server_name_strips_ipv6_brackets() {
        let name = |uri: &str| ip_literal_tls_server_name(&uri.parse::<Uri>().unwrap());

        assert_eq!(
            name("https://[fdaa:0:1:a7b::2]:9000/"),
            Some("fdaa:0:1:a7b::2".to_string())
        );
        assert_eq!(name("https://10.0.0.1:9000/"), None);
        assert_eq!(name("https://golem-shard-manager:9000/"), None);
    }

    fn parse_enabled(value: serde_json::Value) -> EnabledGrpcClientTlsConfig {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn client_tls_settings_accept_inline_values_as_before() {
        let config = parse_enabled(serde_json::json!({
            "client_cert": "",
            "client_key": "",
            "server_ca_cert": "",
        }));
        let sources = config.sources().unwrap();
        assert!(!sources.cert.is_file());
        assert!(!sources.key.is_file());
        assert!(!sources.ca.is_file());
    }

    #[test]
    fn client_tls_settings_accept_files_and_refuse_both() {
        let files = parse_enabled(serde_json::json!({
            "client_cert_file": "/x/tls.crt",
            "client_key_file": "/x/tls.key",
            "server_ca_cert_file": "/x/ca.crt",
        }));
        assert!(files.sources().unwrap().key.is_file());
        assert!(
            files
                .to_safe_string()
                .contains("client_key_file: /x/tls.key")
        );

        let both = parse_enabled(serde_json::json!({
            "client_cert": "",
            "client_key": "",
            "client_key_file": "/x/tls.key",
            "server_ca_cert": "",
        }));
        assert!(matches!(
            both.sources(),
            Err(crate::tls::pem::TlsError::BothSet { name }) if name == "client_key"
        ));
    }
}
