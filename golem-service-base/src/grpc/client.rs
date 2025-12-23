// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use golem_common::retries::RetryState;
use http::Uri;
use scc::hash_map::Entry;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Code, Status};
use tonic_tracing_opentelemetry::middleware::client::{OtelGrpcLayer, OtelGrpcService};
use tower::ServiceBuilder;
use tracing::{Instrument, debug, debug_span, warn};

#[derive(Clone)]
pub struct GrpcClient<T: Clone> {
    endpoint: Uri,
    config: GrpcClientConfig,
    client: Arc<Mutex<Option<GrpcClientConnection<T>>>>,
    client_factory: Arc<dyn Fn(OtelGrpcService<Channel>) -> T + Send + Sync + 'static>,
    target_name: String,
}

impl<T: Clone> GrpcClient<T> {
    pub fn new(
        target_name: impl AsRef<str>,
        client_factory: impl Fn(OtelGrpcService<Channel>) -> T + Send + Sync + 'static,
        endpoint: Uri,
        config: GrpcClientConfig,
    ) -> Self {
        Self {
            target_name: target_name.as_ref().to_string(),
            endpoint,
            config,
            client: Arc::new(Mutex::new(None)),
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
            let mut entry = self
                .get()
                .await
                .map_err(|err| Status::from_error(Box::new(err)))?;
            match f(&mut entry.client).instrument(span.clone()).await {
                Ok(result) => break Ok(result),
                Err(e) => {
                    if requires_reconnect(&e) {
                        let _ = self.client.lock().await.take();
                        if !retries.failed_attempt().await {
                            span.in_scope(|| {
                                warn!("gRPC call failed: {:?}, no more retries", e);
                            });
                            break Err(e);
                        } else {
                            span.in_scope(|| {
                                debug!("gRPC call failed with {:?}, retrying", e);
                            });
                            continue; // retry
                        }
                    } else {
                        span.in_scope(|| {
                            warn!("gRPC call failed: {:?}, not retriable", e);
                        });
                        break Err(e);
                    }
                }
            }
        }
    }

    async fn get(&self) -> Result<GrpcClientConnection<T>, tonic::transport::Error> {
        let mut entry = self.client.lock().await;

        match &*entry {
            Some(client) => Ok(client.clone()),
            None => {
                let mut endpoint = Endpoint::new(self.endpoint.clone())?
                    .connect_timeout(self.config.connect_timeout);

                if let GrpcClientTlsConfig::Enabled(tls) = &self.config.tls {
                    endpoint = endpoint.tls_config(tls.to_tonic())?;
                }

                let channel = endpoint.connect_lazy();
                let channel = ServiceBuilder::new().layer(OtelGrpcLayer).service(channel);
                let client = (self.client_factory)(channel);
                let connection = GrpcClientConnection { client };
                *entry = Some(connection.clone());
                Ok(connection)
            }
        }
    }
}

#[derive(Clone)]
pub struct MultiTargetGrpcClient<T: Clone> {
    config: GrpcClientConfig,
    clients: Arc<scc::HashMap<Uri, GrpcClientConnection<T>>>,
    client_factory: Arc<dyn Fn(OtelGrpcService<Channel>) -> T + Send + Sync>,
    target_name: String,
}

impl<T: Clone> MultiTargetGrpcClient<T> {
    pub fn new(
        target_name: impl AsRef<str>,
        client_factory: impl Fn(OtelGrpcService<Channel>) -> T + Send + Sync + 'static,
        config: GrpcClientConfig,
    ) -> Self {
        Self {
            config,
            clients: Arc::new(scc::HashMap::new()),
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
            let mut entry = self
                .get(endpoint.clone())
                .await
                .map_err(|err| Status::from_error(Box::new(err)))?;
            match f(&mut entry.client).instrument(span.clone()).await {
                Ok(result) => break Ok(result),
                Err(e) => {
                    if requires_reconnect(&e) {
                        self.clients.remove_async(&endpoint).await;
                        if !retries.failed_attempt().await {
                            span.in_scope(|| {
                                warn!("gRPC call failed: {:?}, no more retries", e);
                            });
                            break Err(e);
                        } else {
                            span.in_scope(|| {
                                debug!("gRPC call failed with {:?}, retrying", e);
                            });
                            continue; // retry
                        }
                    } else {
                        span.in_scope(|| {
                            warn!("gRPC call failed: {:?}, not retriable", e);
                        });
                        break Err(e);
                    }
                }
            }
        }
    }

    pub fn uses_tls(&self) -> bool {
        self.config.tls_enabled()
    }

    async fn get(&self, endpoint: Uri) -> Result<GrpcClientConnection<T>, tonic::transport::Error> {
        let connect_timeout = self.config.connect_timeout;
        let entry = self.clients.entry_async(endpoint.clone()).await;
        match entry {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                let mut endpoint = Endpoint::new(endpoint)?.connect_timeout(connect_timeout);

                if let GrpcClientTlsConfig::Enabled(tls) = &self.config.tls {
                    endpoint = endpoint.tls_config(tls.to_tonic())?;
                }

                let channel = endpoint.connect_lazy();
                let channel = ServiceBuilder::new().layer(OtelGrpcLayer).service(channel);
                let client = (self.client_factory)(channel);
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
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
    pub retries_on_unavailable: RetryConfig,
    pub tls: GrpcClientTlsConfig,
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
            retries_on_unavailable: RetryConfig::default(),
            tls: GrpcClientTlsConfig::Disabled(Empty {}),
        }
    }
}

impl SafeDisplay for GrpcClientConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "connect_timeout: {:?}", self.connect_timeout);
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
    e.code() == Code::Unavailable
}
