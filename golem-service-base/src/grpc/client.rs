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

use golem_common::SafeDisplay;
use golem_common::model::base64::Base64;
use golem_common::model::{Empty, RetryConfig};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use golem_common::retries::RetryState;
use http::Uri;
use scc::hash_map::Entry;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Code, Status};
use tonic_tracing_opentelemetry::middleware::client::{OtelGrpcLayer, OtelGrpcService};
use tower::ServiceBuilder;
use tracing::{Instrument, debug, debug_span, warn};

use crate::metrics::grpc::{
    record_internal_grpc_failure, record_internal_grpc_retry, record_internal_grpc_success,
};

fn build_endpoint(uri: Uri, config: &GrpcClientConfig) -> Result<Endpoint, tonic::transport::Error> {
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

    if config.tcp_keepalive.is_some() {
        endpoint = endpoint.tcp_keepalive(config.tcp_keepalive);
    }

    if let Some(buffer_size) = config.buffer_size {
        endpoint = endpoint.buffer_size(buffer_size);
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
/// after which they all fail and the caller can re-resolve. See
/// tests/grpc_client.rs::concurrent_calls_to_unreachable_peer_fail_within_one_connect_timeout.
type SharedConnect = Shared<BoxFuture<'static, Result<Channel, Status>>>;

fn connect_shared(endpoint: Endpoint, connect_timeout: Duration) -> SharedConnect {
    async move {
        match tokio::time::timeout(connect_timeout, endpoint.connect()).await {
            Ok(Ok(channel)) => Ok(channel),
            Ok(Err(err)) => Err(Status::unavailable(format!("tcp connect error: {err}"))),
            Err(_) => Err(Status::unavailable(format!(
                "connection not established within {connect_timeout:?}"
            ))),
        }
    }
    .boxed()
    .shared()
}

#[derive(Clone)]
pub struct GrpcClient<T: Clone> {
    endpoint: Uri,
    config: GrpcClientConfig,
    client: Arc<Mutex<Option<GrpcClientConnection<T>>>>,
    connecting: Arc<Mutex<Option<SharedConnect>>>,
    client_factory: Arc<dyn Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync + 'static>,
    target_name: String,
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
            connecting: Arc::new(Mutex::new(None)),
            client_factory: Arc::new(client_factory),
        }
    }

    pub async fn call<F, R>(&self, description: impl AsRef<str>, f: F) -> Result<R, Status>
    where
        F: for<'a> Fn(&'a mut T) -> Pin<Box<dyn Future<Output = Result<R, Status>> + 'a + Send>>
            + Send,
    {
        let mut retries = RetryState::new(&self.config.retries_on_unavailable);
        let span = debug_span!(
            "gRPC call",
            target_name = self.target_name,
            description = description.as_ref()
        );
        loop {
            retries.start_attempt();
            // A failed connection attempt goes through the same retry path as a
            // failed call, so `retries_on_unavailable` still governs it.
            let mut entry = match self.get().await {
                Ok(entry) => entry,
                Err(e) => {
                    if !retries.failed_attempt().await {
                        span.in_scope(|| warn!("gRPC connect failed: {:?}, no more retries", e));
                        record_internal_grpc_failure(&self.target_name, description.as_ref(), &e);
                        break Err(e);
                    }
                    record_internal_grpc_retry(&self.target_name, description.as_ref());
                    continue;
                }
            };
            let attempt_start = Instant::now();
            match f(&mut entry.client).instrument(span.clone()).await {
                Ok(result) => {
                    record_internal_grpc_success(
                        &self.target_name,
                        description.as_ref(),
                        attempt_start.elapsed(),
                    );
                    break Ok(result);
                }
                Err(e) => {
                    if requires_reconnect(&e) {
                        let _ = self.client.lock().await.take();
                        if !retries.failed_attempt().await {
                            span.in_scope(|| {
                                warn!("gRPC call failed: {:?}, no more retries", e);
                            });
                            record_internal_grpc_failure(
                                &self.target_name,
                                description.as_ref(),
                                &e,
                            );
                            break Err(e);
                        } else {
                            span.in_scope(|| {
                                debug!("gRPC call failed with {:?}, retrying", e);
                            });
                            record_internal_grpc_retry(&self.target_name, description.as_ref());
                            continue; // retry
                        }
                    } else {
                        span.in_scope(|| {
                            warn!("gRPC call failed: {:?}, not retriable", e);
                        });
                        record_internal_grpc_failure(&self.target_name, description.as_ref(), &e);
                        break Err(e);
                    }
                }
            }
        }
    }

    async fn get(&self) -> Result<GrpcClientConnection<T>, Status> {
        if let Some(connection) = self.client.lock().await.clone() {
            return Ok(connection);
        }

        // One connection attempt is shared by everyone waiting for it.
        let shared = {
            let mut connecting = self.connecting.lock().await;
            match &*connecting {
                Some(shared) => shared.clone(),
                None => {
                    let endpoint = build_endpoint(self.endpoint.clone(), &self.config)
                        .map_err(|err| Status::from_error(Box::new(err)))?;
                    let shared = connect_shared(endpoint, self.config.connect_timeout);
                    *connecting = Some(shared.clone());
                    shared
                }
            }
        };

        let result = shared.await;
        // Clear it so a later call starts a fresh attempt rather than replaying
        // this one's outcome.
        let _ = self.connecting.lock().await.take();
        let channel = result?;

        let mut entry = self.client.lock().await;
        if let Some(connection) = &*entry {
            return Ok(connection.clone());
        }
        let channel = ServiceBuilder::new().layer(OtelGrpcLayer).service(channel);
        let client = (self.client_factory)(channel, self.config.max_message_size);
        let connection = GrpcClientConnection { client };
        *entry = Some(connection.clone());
        Ok(connection)
    }
}

#[derive(Clone)]
pub struct MultiTargetGrpcClient<T: Clone> {
    config: GrpcClientConfig,
    clients: Arc<scc::HashMap<Uri, GrpcClientConnection<T>>>,
    connecting: Arc<scc::HashMap<Uri, SharedConnect>>,
    client_factory: Arc<dyn Fn(OtelGrpcService<Channel>, usize) -> T + Send + Sync>,
    target_name: String,
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
            connecting: Arc::new(scc::HashMap::new()),
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
        let mut retries = RetryState::new(&self.config.retries_on_unavailable);
        let span = debug_span!(
            "gRPC call",
            target_name = self.target_name,
            endpoint = endpoint.to_string(),
            description = description.as_ref()
        );
        loop {
            retries.start_attempt();
            // A failed connection attempt goes through the same retry path as a
            // failed call, so `retries_on_unavailable` still governs it.
            let mut entry = match self.get(endpoint.clone()).await {
                Ok(entry) => entry,
                Err(e) => {
                    if !retries.failed_attempt().await {
                        span.in_scope(|| warn!("gRPC connect failed: {:?}, no more retries", e));
                        record_internal_grpc_failure(&self.target_name, description.as_ref(), &e);
                        break Err(e);
                    }
                    record_internal_grpc_retry(&self.target_name, description.as_ref());
                    continue;
                }
            };
            let attempt_start = Instant::now();
            match f(&mut entry.client).instrument(span.clone()).await {
                Ok(result) => {
                    record_internal_grpc_success(
                        &self.target_name,
                        description.as_ref(),
                        attempt_start.elapsed(),
                    );
                    break Ok(result);
                }
                Err(e) => {
                    if requires_reconnect(&e) {
                        self.clients.remove_async(&endpoint).await;
                        if !retries.failed_attempt().await {
                            span.in_scope(|| {
                                warn!("gRPC call failed: {:?}, no more retries", e);
                            });
                            record_internal_grpc_failure(
                                &self.target_name,
                                description.as_ref(),
                                &e,
                            );
                            break Err(e);
                        } else {
                            span.in_scope(|| {
                                debug!("gRPC call failed with {:?}, retrying", e);
                            });
                            record_internal_grpc_retry(&self.target_name, description.as_ref());
                            continue; // retry
                        }
                    } else {
                        span.in_scope(|| {
                            warn!("gRPC call failed: {:?}, not retriable", e);
                        });
                        record_internal_grpc_failure(&self.target_name, description.as_ref(), &e);
                        break Err(e);
                    }
                }
            }
        }
    }

    pub fn uses_tls(&self) -> bool {
        self.config.tls_enabled()
    }

    async fn get(&self, endpoint: Uri) -> Result<GrpcClientConnection<T>, Status> {
        if let Some(existing) = self.clients.get_async(&endpoint).await {
            return Ok(existing.get().clone());
        }

        // One connection attempt per target, shared by everyone waiting for it.
        // Connecting outside the map's entry lock also keeps unrelated targets
        // that hash to the same bucket from serialising behind it.
        let shared = match self.connecting.entry_async(endpoint.clone()).await {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let built = build_endpoint(endpoint.clone(), &self.config)
                    .map_err(|err| Status::from_error(Box::new(err)))?;
                let shared = connect_shared(built, self.config.connect_timeout);
                entry.insert_entry(shared.clone());
                shared
            }
        };

        let result = shared.await;
        // Clear it so a later call starts a fresh attempt rather than replaying
        // this one's outcome.
        let _ = self.connecting.remove_async(&endpoint).await;
        let channel = result?;

        match self.clients.entry_async(endpoint).await {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                let channel = ServiceBuilder::new().layer(OtelGrpcLayer).service(channel);
                let client = (self.client_factory)(channel, self.config.max_message_size);
                let connection = GrpcClientConnection { client };
                entry.insert_entry(connection.clone());
                Ok(connection)
            }
        }
    }
}

#[derive(Clone)]
pub struct GrpcClientConnection<T: Clone> {
    client: T,
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
    #[serde(default, with = "humantime_serde::option")]
    pub http2_keep_alive_interval: Option<Duration>,
    /// How long to wait for the PING ack before considering the connection dead.
    #[serde(default, with = "humantime_serde::option")]
    pub http2_keep_alive_timeout: Option<Duration>,
    /// Whether to keep pinging when no requests are in flight.
    #[serde(default)]
    pub http2_keep_alive_while_idle: Option<bool>,
    #[serde(default, with = "humantime_serde::option")]
    pub tcp_keepalive: Option<Duration>,
    /// Depth of tonic's request buffer.
    ///
    /// Note this does NOT bound the stall described on `connect_timeout`:
    /// `tower::Buffer` applies backpressure rather than rejecting, so callers
    /// wait on readiness however shallow the buffer is. Verified with
    /// buffer_size = 1: eight concurrent calls still took 1x..8x connect_timeout.
    #[serde(default)]
    pub buffer_size: Option<usize>,
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
            tcp_keepalive: None,
            buffer_size: None,
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
        let _ = writeln!(&mut result, "tcp_keepalive: {:?}", self.tcp_keepalive);
        let _ = writeln!(&mut result, "buffer_size: {:?}", self.buffer_size);
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

fn requires_reconnect(e: &Status) -> bool {
    if e.code() == Code::Unavailable {
        return true;
    }

    // A dead connection surfaces as a transport error, which tonic reports as
    // `Unknown` rather than `Unavailable`. Matching only on `Unavailable` left
    // such channels cached indefinitely, so every later request queued onto a
    // connection that could never work again.
    std::error::Error::source(e)
        .map(|source| source.is::<tonic::transport::Error>())
        .unwrap_or(false)
}
