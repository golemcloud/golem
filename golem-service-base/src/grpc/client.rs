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
use tokio::sync::Mutex;
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
type SharedConnect = Shared<BoxFuture<'static, Result<Connected, Status>>>;

/// How long a connection to a target nobody is calling is kept.
///
/// Executor pods come and go, and a target that is never called again would
/// otherwise hold its `Channel` — and the `Buffer` worker task tower spawned for
/// it — for the life of the process. Reaching a dropped target again costs one
/// reconnect, which is why this is generous rather than tight.
const IDLE_CONNECTION_TTL: Duration = Duration::from_secs(600);

/// Milliseconds since this process started.
///
/// Monotonic on purpose. An `Instant` cannot live in an atomic, and a wall clock
/// that steps backwards would retire connections that are in constant use.
fn elapsed_millis() -> u64 {
    static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
    EPOCH.elapsed().as_millis() as u64
}

/// When a connection was last handed to a caller.
///
/// Shared between the connection and every client holding it, so the sweep and
/// the callers cannot disagree about whether it is still in use.
#[derive(Clone)]
struct LastUsed(Arc<AtomicU64>);

impl LastUsed {
    fn now() -> Self {
        Self(Arc::new(AtomicU64::new(elapsed_millis())))
    }

    fn touch(&self) {
        self.0.store(elapsed_millis(), Ordering::Relaxed);
    }

    fn idle_for(&self) -> Duration {
        Duration::from_millis(elapsed_millis().saturating_sub(self.0.load(Ordering::Relaxed)))
    }
}

/// An established connection, and what tells it apart from the one that replaces
/// it.
#[derive(Clone)]
struct Connected {
    channel: Channel,
    /// Distinguishes this connection from its successor, so a call that fails
    /// late cannot evict a replacement it never used.
    id: u64,
    /// Cancelled when this connection is retired.
    ///
    /// A cached `Channel` reconnects on its own, one queued request at a time,
    /// and nothing above it can see those requests to share an attempt between
    /// them. Releasing them at the moment the connection is known dead is what
    /// keeps a cohort to one `connect_timeout` between them.
    retired: CancellationToken,
    last_used: LastUsed,
}

fn connect_shared(endpoint: Endpoint, connect_timeout: Duration) -> SharedConnect {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    async move {
        match tokio::time::timeout(connect_timeout, endpoint.connect()).await {
            Ok(Ok(channel)) => Ok(Connected {
                channel,
                id,
                retired: CancellationToken::new(),
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
#[derive(Clone)]
struct Connections {
    by_target: Arc<scc::HashMap<Uri, SharedConnect>>,
}

impl Connections {
    fn new() -> Self {
        Self {
            by_target: Arc::new(scc::HashMap::new()),
        }
    }

    /// Connects to `target`, joining an attempt already under way if there is one.
    async fn connect(&self, target: Uri, config: &GrpcClientConfig) -> Result<Connected, Status> {
        // A shared read first, before anything is built. Every caller but the one
        // that starts an attempt gets its answer here, and `build_endpoint` is
        // not free: with TLS enabled it parses the CA, client certificate and key
        // to build a fresh rustls config each time, which measures about 17.9us
        // against 94ns without. During the reconnect storm this type exists to
        // handle, that is the whole waiting cohort paying for a connection one of
        // them is already making.
        if let Some(existing) = self
            .by_target
            .read_async(&target, |_, attempt| attempt.clone())
            .await
        {
            match existing.peek() {
                // Already connected.
                Some(Ok(connected)) if !connected.retired.is_cancelled() => {
                    connected.last_used.touch();
                    return Ok(connected.clone());
                }
                // Someone else is connecting; wait on theirs. What that
                // resolves to is deliberately not weighed against `retired`: a
                // waiter joining late can be handed a connection an earlier one
                // has already used and retired, and `install` is where that is
                // caught, under the lock that makes catching it worth anything.
                None => return existing.await,
                // A failed attempt is not worth inheriting, and neither is a
                // retired one: `retire` cancels before it forgets, so between
                // those two moments this is still the cached answer and it
                // describes a connection that is gone. Fall through and start a
                // fresh one below.
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
                Some(Ok(connected)) if !connected.retired.is_cancelled() => {
                    connected.last_used.touch();
                    return Ok(connected.clone());
                }
                None => entry.get().clone(),
                // A failed attempt describes a connection nobody is still making
                // and a retired one describes a connection that is gone, so the
                // caller gets its own rather than inheriting either verdict.
                Some(_) => {
                    let attempt = connect_shared(endpoint, config.connect_timeout);
                    *entry.get_mut() = attempt.clone();
                    self.drive(target.clone(), attempt.clone());
                    attempt
                }
            },
            Entry::Vacant(entry) => {
                let attempt = connect_shared(endpoint, config.connect_timeout);
                entry.insert_entry(attempt.clone());
                self.drive(target.clone(), attempt.clone());
                attempt
            }
        };

        attempt.await
    }

    /// Drives an attempt to completion independently of its callers, and clears
    /// it if it failed. Called wherever an attempt is created, while the entry
    /// holding it is still locked, so that no arm can create one and leave it
    /// with nobody to finish it. Only spawning happens here, so holding the
    /// entry across it costs nothing.
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
    fn drive(&self, target: Uri, attempt: SharedConnect) -> JoinHandle<()> {
        let by_target = self.by_target.clone();
        tokio::spawn(async move {
            if attempt.clone().await.is_err() {
                by_target
                    .remove_if_async(&target, |current| current.ptr_eq(&attempt))
                    .await;
            }
        })
    }

    /// Drops connections to targets nobody has called for `idle_ttl`.
    ///
    /// An attempt still in flight is never dropped: it has no outcome to have
    /// been idle with, and callers are waiting on it right now.
    async fn prune(&self, idle_ttl: Duration) {
        self.by_target
            .retain_async(|_, attempt| match attempt.peek() {
                Some(Ok(connected)) => connected.last_used.idle_for() < idle_ttl,
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

/// Test-only seam: a hook awaited inside `connected_client` between a connection
/// being established and being installed, so a test can deterministically retire
/// it in that window. Same pattern as `Cache::evict_interleave` in
/// `golem-common/src/cache.rs`.
#[cfg(test)]
type InstallInterleaveHook = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

#[derive(Clone)]
pub struct GrpcClient<T: Clone> {
    endpoint: Uri,
    config: GrpcClientConfig,
    client: Arc<Mutex<Option<GrpcClientConnection<T>>>>,
    connections: Connections,
    client_factory: Arc<dyn Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync + 'static>,
    target_name: String,
    #[cfg(test)]
    install_interleave: Arc<std::sync::Mutex<Option<InstallInterleaveHook>>>,
}

impl<T: Clone> GrpcClient<T> {
    pub fn new(
        target_name: impl AsRef<str>,
        client_factory: impl Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync + 'static,
        endpoint: Uri,
        config: GrpcClientConfig,
    ) -> Self {
        Self {
            target_name: target_name.as_ref().to_string(),
            endpoint,
            config,
            client: Arc::new(Mutex::new(None)),
            connections: Connections::new(),
            client_factory: Arc::new(client_factory),
            #[cfg(test)]
            install_interleave: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub async fn call<F, R>(&self, description: impl AsRef<str>, f: F) -> Result<R, Status>
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
            let mut entry = match self.connected_client().await {
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
                    self.retire(&entry, &e).await;
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

    /// Returns a connected client, establishing the connection if there is not
    /// one yet. This performs I/O — bounded by `connect_timeout` — in the same
    /// spirit as `RedisLabelledApi::ensure_connected`: nothing connects at
    /// construction, and the first user to need a connection makes it while
    /// everyone else waits on that same attempt.
    async fn connected_client(&self) -> Result<GrpcClientConnection<T>, Status> {
        // A connection already known dead is skipped rather than handed back.
        // `retire` cancels before it clears the cache, and cancelling is what
        // wakes every request parked on the connection, so those callers arrive
        // back here inside the window `retire` has not finished closing.
        if let Some(connection) = self
            .client
            .lock()
            .await
            .clone()
            .filter(|connection| !connection.retired.is_cancelled())
        {
            connection.last_used.touch();
            return Ok(connection);
        }

        let connected = self
            .connections
            .connect(self.endpoint.clone(), &self.config)
            .await?;

        let mut entry = self.client.lock().await;
        if let Some(connection) = entry
            .as_ref()
            .filter(|connection| !connection.retired.is_cancelled())
        {
            return Ok(connection.clone());
        }
        let connection = self.install(&connected).await?;
        *entry = Some(connection.clone());
        Ok(connection)
    }

    /// See [`MultiTargetGrpcClient::install`].
    async fn install(&self, connected: &Connected) -> Result<GrpcClientConnection<T>, Status> {
        #[cfg(test)]
        self.install_interleave().await;

        if connected.retired.is_cancelled() {
            return Err(Status::unavailable("connection retired before it was used"));
        }
        let channel = ServiceBuilder::new()
            .layer(OtelGrpcLayer)
            .service(connected.channel.clone());
        Ok(GrpcClientConnection {
            client: (self.client_factory)(channel, self.config.max_message_size),
            id: connected.id,
            retired: connected.retired.clone(),
            last_used: connected.last_used.clone(),
        })
    }

    /// Test-only: installs the hook awaited between a connection being
    /// established and being installed.
    #[cfg(test)]
    fn set_install_interleave(&self, hook: InstallInterleaveHook) {
        *self.install_interleave.lock().unwrap() = Some(hook);
    }

    #[cfg(test)]
    async fn install_interleave(&self) {
        let hook = self.install_interleave.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook().await;
        }
    }

    /// See [`MultiTargetGrpcClient::retire`].
    async fn retire(&self, connection: &GrpcClientConnection<T>, cause: &Status) {
        if connection_is_gone(cause) {
            connection.retired.cancel();
        }
        {
            let mut cached = self.client.lock().await;
            if cached.as_ref().is_some_and(|held| held.id == connection.id) {
                *cached = None;
            }
        }
        self.connections.forget(&self.endpoint, connection.id).await;
    }
}

#[derive(Clone)]
pub struct MultiTargetGrpcClient<T: Clone> {
    config: GrpcClientConfig,
    clients: Arc<scc::HashMap<Uri, GrpcClientConnection<T>>>,
    connections: Connections,
    client_factory: Arc<dyn Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync>,
    target_name: String,
    #[cfg(test)]
    install_interleave: Arc<std::sync::Mutex<Option<InstallInterleaveHook>>>,
}

impl<T: Clone> MultiTargetGrpcClient<T> {
    pub fn new(
        target_name: impl AsRef<str>,
        client_factory: impl Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync + 'static,
        config: GrpcClientConfig,
    ) -> Self {
        Self {
            config,
            clients: Arc::new(scc::HashMap::new()),
            connections: Connections::new(),
            client_factory: Arc::new(client_factory),
            target_name: target_name.as_ref().to_string(),
            #[cfg(test)]
            install_interleave: Arc::new(std::sync::Mutex::new(None)),
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
            let mut entry = match self.connected_client(endpoint.clone()).await {
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

    /// Returns a connected client for `endpoint`, establishing the connection if
    /// there is not one yet. This performs I/O — bounded by `connect_timeout` —
    /// in the same spirit as `RedisLabelledApi::ensure_connected`: nothing
    /// connects at construction, and the first caller to need a connection to a
    /// given target makes it while everyone else waits on that same attempt.
    async fn connected_client(&self, endpoint: Uri) -> Result<GrpcClientConnection<T>, Status> {
        // A shared read rather than `get_async`, which takes the bucket's writer
        // lock. This is the hot path: every call to an already-connected target
        // runs it, and the entry is only ever read here.
        if let Some(existing) = self
            .clients
            .read_async(&endpoint, |_, client| client.clone())
            .await
            // A connection already known dead is skipped rather than handed
            // back. `retire` cancels before it clears the cache, and cancelling
            // is what wakes every request parked on the connection, so that
            // whole cohort arrives back here inside the window `retire` has not
            // finished closing.
            .filter(|existing| !existing.retired.is_cancelled())
        {
            existing.last_used.touch();
            return Ok(existing);
        }

        let connected = self
            .connections
            .connect(endpoint.clone(), &self.config)
            .await?;

        // A target that was already cached never reaches here, so this is the one
        // place these maps grow and the right place to drop what has gone cold. A
        // client whose set of targets has settled sweeps not at all; one watching
        // pods come and go sweeps each time one appears.
        self.clients
            .retain_async(|_, cached| cached.last_used.idle_for() < IDLE_CONNECTION_TTL)
            .await;
        self.connections.prune(IDLE_CONNECTION_TTL).await;

        match self.clients.entry_async(endpoint).await {
            Entry::Occupied(mut entry) => {
                if !entry.get().retired.is_cancelled() {
                    return Ok(entry.get().clone());
                }
                let connection = self.install(&connected).await?;
                *entry.get_mut() = connection.clone();
                Ok(connection)
            }
            Entry::Vacant(entry) => {
                let connection = self.install(&connected).await?;
                entry.insert_entry(connection.clone());
                Ok(connection)
            }
        }
    }

    /// Builds the client for a connection about to be cached, refusing one that
    /// has been retired since it was established. Caching that would hand every
    /// later caller a connection failing the instant it is used, until one of
    /// them retires it again.
    ///
    /// Both callers run this while holding the lock `retire` needs to clear the
    /// cache, and `retire` cancels before it takes that lock. So either the
    /// cancel lands first and is read here, or it lands afterwards and `retire`
    /// clears what was installed. Reading it before taking the lock leaves an
    /// interleaving where neither happens.
    async fn install(&self, connected: &Connected) -> Result<GrpcClientConnection<T>, Status> {
        #[cfg(test)]
        self.install_interleave().await;

        if connected.retired.is_cancelled() {
            return Err(Status::unavailable("connection retired before it was used"));
        }
        let channel = ServiceBuilder::new()
            .layer(OtelGrpcLayer)
            .service(connected.channel.clone());
        Ok(GrpcClientConnection {
            client: (self.client_factory)(channel, self.config.max_message_size),
            id: connected.id,
            retired: connected.retired.clone(),
            last_used: connected.last_used.clone(),
        })
    }

    /// Test-only: installs the hook awaited between a connection being
    /// established and being installed.
    #[cfg(test)]
    fn set_install_interleave(&self, hook: InstallInterleaveHook) {
        *self.install_interleave.lock().unwrap() = Some(hook);
    }

    #[cfg(test)]
    async fn install_interleave(&self) {
        let hook = self.install_interleave.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook().await;
        }
    }

    /// Retires the connection a failed call was riding: clears it from the caches
    /// so the next call builds a new one and, when the transport is what failed,
    /// releases every other request still on it.
    ///
    /// Keyed on the connection's identity rather than on the target alone. Two
    /// callers can fail on the same connection at different moments, and without
    /// that the later failure would evict the replacement the earlier one had
    /// already established.
    ///
    /// A status the peer sent back is proof the transport works, however unwelcome
    /// the answer, so that case evicts the connection without disturbing the
    /// requests already riding it. Only a connection that is gone justifies
    /// cutting those short.
    async fn retire(&self, endpoint: &Uri, connection: &GrpcClientConnection<T>, cause: &Status) {
        if connection_is_gone(cause) {
            connection.retired.cancel();
        }
        self.clients
            .remove_if_async(endpoint, |cached| cached.id == connection.id)
            .await;
        self.connections.forget(endpoint, connection.id).await;
    }
}

#[derive(Clone)]
pub struct GrpcClientConnection<T: Clone> {
    client: T,
    id: u64,
    retired: CancellationToken,
    last_used: LastUsed,
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
/// retired.
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
    tokio::select! {
        // A result that is already there wins over a retirement landing in the
        // same tick.
        biased;
        result = f(&mut connection.client).instrument(attempts.span()) => result,
        () = connection.retired.cancelled() => Err(Status::unavailable(
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

    if e.code() == Code::Unavailable {
        return true;
    }

    // A dead connection surfaces as a transport error, which tonic reports as
    // `Unknown` or `Cancelled` rather than `Unavailable`. Matching only on
    // `Unavailable` left such channels cached indefinitely, so every later
    // request queued onto a connection that could never work again.
    has_transport_source(e)
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
    use tokio::sync::Notify;

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

    /// Two callers can fail on the same connection at different moments. The
    /// later failure must not discard the replacement the earlier one's retry has
    /// already established, which is what keying eviction on the target alone
    /// used to do: the third caller then opened a third connection.
    #[test]
    async fn forget_only_removes_the_connection_it_names() {
        let (target, _peer) = silent_peer().await;
        let config = GrpcClientConfig::default();
        let connections = Connections::new();

        let first = connections.connect(target.clone(), &config).await.unwrap();

        // A failure arriving late, naming a connection that has already been
        // replaced.
        connections.forget(&target, first.id + 1).await;
        let reused = connections.connect(target.clone(), &config).await.unwrap();
        assert_eq!(
            reused.id, first.id,
            "forget discarded a connection whose identity it was not given"
        );

        // The connection that actually failed does go.
        connections.forget(&target, first.id).await;
        let replacement = connections.connect(target.clone(), &config).await.unwrap();
        assert_ne!(
            replacement.id, first.id,
            "forget left in place the very connection it was given"
        );
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

    fn counting_single_target_client(endpoint: Uri) -> (GrpcClient<()>, Arc<AtomicU64>) {
        let built = Arc::new(AtomicU64::new(0));
        let counter = built.clone();
        let client = GrpcClient::new(
            "test",
            move |_, _| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
            endpoint,
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

    /// Builds a hook that parks `connected_client` between establishing a
    /// connection and installing it. Returns it with the notify it signals on
    /// arrival and the notify that lets it go on again.
    fn park_at_install() -> (InstallInterleaveHook, Arc<Notify>, Arc<Notify>) {
        let at_seam = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let (arrived, go) = (at_seam.clone(), resume.clone());
        let hook: InstallInterleaveHook = Arc::new(move || {
            let (arrived, go) = (arrived.clone(), go.clone());
            async move {
                arrived.notify_one();
                go.notified().await;
            }
            .boxed()
        });
        (hook, at_seam, resume)
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

        let first = client.connected_client(target.clone()).await.unwrap();
        client.retire(&target, &first, &cause).await;

        let replacement = client.connected_client(target.clone()).await.unwrap();
        assert_ne!(
            replacement.id, first.id,
            "the first failure evicted nothing"
        );

        // A second caller, still holding the original, fails late.
        client.retire(&target, &first, &cause).await;
        let handed_out = client.connected_client(target.clone()).await.unwrap();

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

    /// A connection can also be retired while a caller
    /// is partway through installing it. Reading the retirement before taking
    /// the lock `retire` takes let the caller install a connection it had
    /// already been told was dead, and every later caller was handed it in
    /// turn.
    #[test]
    async fn a_connection_retired_while_it_is_being_installed_is_not_cached() {
        let (target, _peer) = silent_peer().await;
        let (client, _built) = counting_client();
        let (hook, at_seam, resume) = park_at_install();
        client.set_install_interleave(hook);

        let installing = tokio::spawn({
            let client = client.clone();
            let target = target.clone();
            async move { client.connected_client(target).await }
        });
        expect(
            "the call never reached the seam between establishing a connection \
             and installing it",
            at_seam.notified(),
        )
        .await;

        // Stand in for a sibling caller failing on this very connection while
        // the one above is still installing it.
        let connected = client
            .connections
            .connect(target.clone(), &client.config)
            .await
            .unwrap();
        connected.retired.cancel();
        client.connections.forget(&target, connected.id).await;

        resume.notify_one();
        let mut installed = None;
        expect("the call never came back after being let go", async {
            installed = Some(installing.await.unwrap());
        })
        .await;
        let installed = installed.unwrap();

        assert!(
            installed.is_err(),
            "a connection retired mid-install was handed to the caller \
             installing it"
        );
        assert!(
            !client.clients.contains_async(&target).await,
            "a retired connection was left in the cache for every later caller"
        );
    }

    /// The single-target client keeps its own connection and its own copy of
    /// that window, so it needs its own proof.
    #[test]
    async fn the_single_target_client_does_not_cache_a_connection_retired_mid_install() {
        let (target, _peer) = silent_peer().await;
        let (client, _built) = counting_single_target_client(target.clone());
        let (hook, at_seam, resume) = park_at_install();
        client.set_install_interleave(hook);

        let installing = tokio::spawn({
            let client = client.clone();
            async move { client.connected_client().await }
        });
        expect(
            "the call never reached the seam between establishing a connection \
             and installing it",
            at_seam.notified(),
        )
        .await;

        let connected = client
            .connections
            .connect(target.clone(), &client.config)
            .await
            .unwrap();
        connected.retired.cancel();
        client.connections.forget(&target, connected.id).await;

        resume.notify_one();
        let mut installed = None;
        expect("the call never came back after being let go", async {
            installed = Some(installing.await.unwrap());
        })
        .await;
        let installed = installed.unwrap();

        assert!(
            installed.is_err(),
            "a connection retired mid-install was handed to the caller \
             installing it"
        );
        assert!(
            client.client.lock().await.is_none(),
            "a retired connection was left in the cache for every later caller"
        );
    }

    /// `retire` cancels a connection before it forgets it, so between those two
    /// moments the cached attempt still answers with a connection that is gone.
    /// Handing it back turns every caller arriving in that window into a failure
    /// that had no reason to happen.
    #[test]
    async fn a_retired_connection_is_not_handed_to_the_next_caller() {
        let (target, _peer) = silent_peer().await;
        let config = GrpcClientConfig::default();
        let connections = Connections::new();

        let first = connections.connect(target.clone(), &config).await.unwrap();
        first.retired.cancel();

        let next = connections.connect(target.clone(), &config).await.unwrap();
        assert_ne!(
            next.id, first.id,
            "a connection that had been retired was handed straight back"
        );
        assert!(
            !next.retired.is_cancelled(),
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
        let connections = Connections::new();

        let first = connections.connect(target.clone(), &config).await.unwrap();

        // Just used, so no sweep should touch it.
        connections.prune(Duration::from_secs(60)).await;
        let kept = connections.connect(target.clone(), &config).await.unwrap();
        assert_eq!(
            kept.id, first.id,
            "the sweep dropped a connection that was still in use"
        );

        // Against a zero TTL everything counts as cold.
        connections.prune(Duration::ZERO).await;
        let replacement = connections.connect(target.clone(), &config).await.unwrap();
        assert_ne!(
            replacement.id, first.id,
            "the sweep kept a connection nobody had called"
        );
    }

    /// An attempt still being made belongs to the callers waiting on it, and has
    /// no last-used moment to be judged by. Sweeping it would send the next caller
    /// off to open a second connection to the same place.
    #[test]
    async fn the_sweep_leaves_an_attempt_in_flight_alone() {
        let config = GrpcClientConfig::default();
        let connections = Connections::new();

        // Nothing listens here, and the connect is held open by a timeout far
        // longer than the test, so the attempt stays unsettled throughout.
        let target: Uri = "http://127.0.0.1:1/".parse().unwrap();
        let endpoint = build_endpoint(target.clone(), &config).unwrap();
        let attempt = connect_shared(endpoint, Duration::from_secs(600));
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
    async fn a_status_from_the_peer_does_not_retire_the_connection() {
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
    async fn only_a_dead_connection_releases_the_requests_riding_it() {
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

    /// The install guard is only worth something if `retire` has already
    /// cancelled by the time an installer holding the lock can read it. `retire`
    /// cancels first and takes the lock second, so a caller holding that lock
    /// must find the connection already retired. Swap that order and the guard
    /// reads live every time, which is the bug it exists to stop.
    #[test]
    async fn retire_cancels_before_it_takes_the_lock_that_clears_the_cache() {
        let (target, _peer) = silent_peer().await;
        let (client, _built) = counting_client();
        let cause = dead_transport_status().await;
        assert!(
            connection_is_gone(&cause),
            "the cause has to be one that retires a connection at all"
        );

        let connection = client.connected_client(target.clone()).await.unwrap();

        // Hold the very lock `retire` has to take to clear the connection.
        let held = client.clients.entry_async(target.clone()).await;
        let retiring = tokio::spawn({
            let (client, target, connection) = (client.clone(), target.clone(), connection.clone());
            async move { client.retire(&target, &connection, &cause).await }
        });

        expect(
            "retire had not cancelled by the time it wanted the lock, so an \
             installer holding the lock would read the connection as live",
            connection.retired.cancelled(),
        )
        .await;

        drop(held);
        retiring.await.unwrap();
    }

    /// The single-target client keeps its connection behind its own lock, so the
    /// same ordering has to hold there too.
    #[test]
    async fn the_single_target_client_retires_in_the_same_order() {
        let (target, _peer) = silent_peer().await;
        let (client, _built) = counting_single_target_client(target.clone());
        let cause = dead_transport_status().await;

        let connection = client.connected_client().await.unwrap();

        let held = client.client.lock().await;
        let retiring = tokio::spawn({
            let (client, connection) = (client.clone(), connection.clone());
            async move { client.retire(&connection, &cause).await }
        });

        expect(
            "retire had not cancelled by the time it wanted the lock, so an \
             installer holding the lock would read the connection as live",
            connection.retired.cancelled(),
        )
        .await;

        drop(held);
        retiring.await.unwrap();
    }

    /// Cancelling is what releases every request parked on a connection, and it
    /// happens before the connection is cleared from the cache. So the whole
    /// cohort `retire` just released comes back for a connection while `retire`
    /// is still clearing, and the cache would hand each of them the same dead
    /// one it had already been told about.
    #[test]
    async fn a_connection_already_known_dead_is_not_handed_out_again() {
        let (target, _peer) = silent_peer().await;
        let (client, built) = counting_client();

        let first = client.connected_client(target.clone()).await.unwrap();
        first.retired.cancel();

        let next = client.connected_client(target.clone()).await.unwrap();
        assert_ne!(
            next.id, first.id,
            "the cache handed back a connection it had already been told was dead"
        );
        assert!(
            !next.retired.is_cancelled(),
            "the connection put in its place came out retired too"
        );
        assert_eq!(
            built.load(Ordering::SeqCst),
            2,
            "the dead connection was replaced without a client being built for \
             the replacement"
        );
    }

    /// The single-target twin of
    /// [`a_connection_already_known_dead_is_not_handed_out_again`]. This client
    /// reads its connection in two places, so both have to skip a dead one.
    #[test]
    async fn the_single_target_client_does_not_hand_out_a_connection_known_dead() {
        let (target, _peer) = silent_peer().await;
        let (client, built) = counting_single_target_client(target);

        let first = client.connected_client().await.unwrap();
        first.retired.cancel();

        let next = client.connected_client().await.unwrap();
        assert_ne!(
            next.id, first.id,
            "the cache handed back a connection it had already been told was dead"
        );
        assert!(
            !next.retired.is_cancelled(),
            "the connection put in its place came out retired too"
        );
        assert_eq!(
            built.load(Ordering::SeqCst),
            2,
            "the dead connection was replaced without a client being built for \
             the replacement"
        );
    }

    /// The single-target twin of
    /// [`a_late_failure_does_not_evict_the_replacement_client`]. Two callers can
    /// fail on the same connection at different moments here as well.
    #[test]
    async fn a_late_failure_does_not_evict_the_replacement_single_target_client() {
        let (target, _peer) = silent_peer().await;
        let (client, built) = counting_single_target_client(target);
        let cause = Status::unavailable("peer went away");

        let first = client.connected_client().await.unwrap();
        client.retire(&first, &cause).await;

        let replacement = client.connected_client().await.unwrap();
        assert_ne!(
            replacement.id, first.id,
            "the first failure evicted nothing"
        );

        // A second caller, still holding the original, fails late.
        client.retire(&first, &cause).await;
        let handed_out = client.connected_client().await.unwrap();

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

    /// `drive` clears an attempt that failed, and a target can have a failed
    /// attempt and its replacement about at the same time. Keyed on the target
    /// alone, the failed attempt's own task would clear the replacement that had
    /// already taken its place.
    #[test]
    async fn a_failed_attempts_own_task_does_not_clear_the_replacement() {
        let (target, _peer) = silent_peer().await;
        let config = GrpcClientConfig::default();
        let connections = Connections::new();

        // Nothing listens here, and no time at all to reach it.
        let endpoint = build_endpoint("http://127.0.0.1:1/".parse().unwrap(), &config).unwrap();
        let failed = connect_shared(endpoint, Duration::ZERO);
        assert!(
            failed.clone().await.is_err(),
            "the attempt this test is about has to be one that failed"
        );

        let replacement = connections.connect(target.clone(), &config).await.unwrap();
        connections.drive(target.clone(), failed).await.unwrap();

        let kept = connections.connect(target.clone(), &config).await.unwrap();
        assert_eq!(
            kept.id, replacement.id,
            "a failed attempt's own task cleared the replacement that had taken \
             its place"
        );
    }
}
