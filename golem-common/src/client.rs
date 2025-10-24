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

use crate::model::RetryConfig;
use crate::retries::RetryState;
use http::Uri;
use scc::hash_map::Entry;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tonic::transport::{Channel, Endpoint};
use tonic::{Code, Status};
use tracing::{debug, debug_span, warn, Instrument};

#[derive(Clone)]
pub struct GrpcClient<T: Clone> {
    endpoint: Uri,
    config: GrpcClientConfig,
    client: Arc<Mutex<Option<GrpcClientConnection<T>>>>,
    client_factory: Arc<dyn Fn(Channel) -> T + Send + Sync + 'static>,
    target_name: String,
}

impl<T: Clone> GrpcClient<T> {
    pub fn new(
        target_name: impl AsRef<str>,
        client_factory: impl Fn(Channel) -> T + Send + Sync + 'static,
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
                let endpoint = Endpoint::new(self.endpoint.clone())?
                    .connect_timeout(self.config.connect_timeout);
                let channel = endpoint.connect_lazy();
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
    client_factory: Arc<dyn Fn(Channel) -> T + Send + Sync>,
    target_name: String,
}

impl<T: Clone> MultiTargetGrpcClient<T> {
    pub fn new(
        target_name: impl AsRef<str>,
        client_factory: impl Fn(Channel) -> T + Send + Sync + 'static,
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

    async fn get(&self, endpoint: Uri) -> Result<GrpcClientConnection<T>, tonic::transport::Error> {
        let connect_timeout = self.config.connect_timeout;
        let entry = self.clients.entry_async(endpoint.clone()).await;
        match entry {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                let endpoint = Endpoint::new(endpoint)?.connect_timeout(connect_timeout);
                let channel = endpoint.connect_lazy();
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

#[derive(Debug, Clone)]
pub struct GrpcClientConfig {
    pub connect_timeout: Duration,
    pub retries_on_unavailable: RetryConfig,
}

impl Default for GrpcClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            retries_on_unavailable: RetryConfig::default(),
        }
    }
}

fn requires_reconnect(e: &Status) -> bool {
    e.code() == Code::Unavailable
}
