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

pub mod api;
pub mod bootstrap;
pub mod config;
pub mod custom_api;
pub mod gateway_server;
pub mod grpcapi;
pub mod ingress;
pub mod invocation_session_token;
pub mod mcp;
pub mod metrics;
pub mod model;
pub mod path;
pub mod public_endpoint;
pub mod service;

use crate::bootstrap::Services;
use crate::config::WorkerServiceConfig;
use crate::ingress::TrustedIngress;
use crate::mcp::{McpOnlyEndpoint, make_mcp_endpoint};
use crate::public_endpoint::PublicEndpoint;
use crate::service::registry_event_subscriber::WorkerServiceRegistryInvalidationHandler;
use anyhow::{Context, anyhow};
use golem_common::poem::{
    CliClientInfoMiddleware, LazyEndpointExt, OpenTelemetryMetrics, OpenTelemetryTracing,
    PrometheusExporter,
};
use opentelemetry_sdk::trace::SdkTracer;
use poem::endpoint::BoxEndpoint;
use poem::listener::Acceptor;
use poem::middleware::{CookieJarManager, Cors};
use poem::{EndpointExt, Route};
use prometheus::Registry;
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::{Instrument, info};

#[cfg(test)]
test_r::enable!();

#[cfg(test)]
mod transport_test_support;

pub struct RunDetails {
    pub http_port: u16,
    pub grpc_port: u16,
    pub custom_request_port: u16,
    pub mcp_port: u16,
}

pub struct TrafficReadyEndpoints {
    pub grpc_port: u16,
    pub custom_request_port: u16,
    pub mcp_port: u16,
    pub api_endpoint: BoxEndpoint<'static>,
    /// The endpoint that the custom request port serves: HTTP API and MCP deployments,
    /// chosen by the request's host. See [`PublicEndpoint`].
    pub public_endpoint: BoxEndpoint<'static>,
    /// Decides which peers may set a request's public origin with `X-Forwarded-*` headers.
    pub trusted_ingress: TrustedIngress,
}

#[derive(Clone)]
pub struct WorkerService {
    config: WorkerServiceConfig,
    prometheus_registry: Registry,
    services: Services,
}

impl WorkerService {
    pub async fn new(
        config: WorkerServiceConfig,
        prometheus_registry: Registry,
    ) -> anyhow::Result<Self> {
        let services: Services = Services::new(&config).await?;

        Ok(Self {
            config,
            prometheus_registry,
            services,
        })
    }

    pub async fn run(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
        tracer: Option<SdkTracer>,
    ) -> anyhow::Result<RunDetails> {
        let registry_service = self.services.registry_service.clone();
        let component_service = self.services.component_service.clone();
        let agent_resolution_cache = self.services.agent_resolution_cache.clone();
        let route_resolver = self.services.route_resolver.clone();
        let auth_service = self.services.auth_service.clone();
        let mcp_capability_lookup = self.services.mcp_capability_lookup.clone();
        join_set.spawn(async move {
            WorkerServiceRegistryInvalidationHandler::run(
                registry_service,
                component_service,
                agent_resolution_cache,
                route_resolver,
                auth_service,
                mcp_capability_lookup,
                None,
            )
            .await;
            Ok(())
        });

        let grpc_port = self.start_grpc_server(join_set).await?;
        let http_port = self.start_http_server(join_set, tracer.clone()).await?;
        let (custom_request_port, _) = self
            .start_api_gateway_server(join_set, tracer.clone())
            .await?;
        let mcp_port = self.start_mcp_server(join_set, tracer).await?;

        info!(
            "Started worker service on ports: http: {}, grpc: {}, gateway: {}, mcp: {}",
            http_port, grpc_port, custom_request_port, mcp_port
        );

        Ok(RunDetails {
            http_port,
            grpc_port,
            custom_request_port,
            mcp_port,
        })
    }

    /// Endpoints are only valid until joinset is dropped
    pub async fn start_endpoints(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
        tracer: Option<SdkTracer>,
    ) -> Result<TrafficReadyEndpoints, anyhow::Error> {
        let registry_service = self.services.registry_service.clone();
        let component_service = self.services.component_service.clone();
        let agent_resolution_cache = self.services.agent_resolution_cache.clone();
        let route_resolver = self.services.route_resolver.clone();
        let auth_service = self.services.auth_service.clone();
        let mcp_capability_lookup = self.services.mcp_capability_lookup.clone();
        join_set.spawn(async move {
            WorkerServiceRegistryInvalidationHandler::run(
                registry_service,
                component_service,
                agent_resolution_cache,
                route_resolver,
                auth_service,
                mcp_capability_lookup,
                None,
            )
            .await;
            Ok(())
        });

        let grpc_port = self.start_grpc_server(join_set).await?;
        let (custom_request_port, public_endpoint) = self
            .start_api_gateway_server(join_set, tracer.clone())
            .await?;
        let mcp_port = self.start_mcp_server(join_set, tracer).await?;
        let api_endpoint = api::make_open_api_service(&self.services).boxed();

        Ok(TrafficReadyEndpoints {
            grpc_port,
            api_endpoint,
            mcp_port,
            custom_request_port,
            public_endpoint: public_endpoint.boxed(),
            trusted_ingress: self.trusted_ingress(),
        })
    }

    async fn start_grpc_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> Result<u16, anyhow::Error> {
        grpcapi::start_grpc_server(
            self.config.socket_addr(self.config.grpc.port),
            &self.config.grpc,
            self.services.clone(),
            join_set,
        )
        .await
        .context("gRPC server failed")
    }

    async fn start_http_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
        tracer: Option<SdkTracer>,
    ) -> Result<u16, anyhow::Error> {
        let api_service = api::make_open_api_service(&self.services);

        let ui = api_service.swagger_ui();
        let spec = api_service.spec_endpoint_yaml();
        let metrics = PrometheusExporter::new(self.prometheus_registry.clone());

        let cors = Cors::new()
            .allow_origin_regex(&self.config.cors_origin_regex)
            .allow_credentials(true);

        let app = Route::new()
            .nest("/", api_service)
            .nest("/docs", ui)
            .nest("/specs", spec)
            .nest("/metrics", metrics)
            .with(CookieJarManager::new())
            .with(cors)
            .with(CliClientInfoMiddleware::new())
            .with_if_lazy(tracer.is_some(), || {
                OpenTelemetryTracing::new(tracer.unwrap())
            });

        let acceptor =
            golem_service_base::net::bind_poem_acceptor(self.config.socket_addr(self.config.port))?;
        let port = acceptor.local_addr()[0]
            .as_socket_addr()
            .expect("socket address")
            .port();

        join_set.spawn(
            async move {
                poem::Server::new_with_acceptor(acceptor)
                    .run(app)
                    .await
                    .map_err(|e| e.into())
            }
            .in_current_span(),
        );

        Ok(port)
    }

    fn trusted_ingress(&self) -> TrustedIngress {
        TrustedIngress::new(&self.config.route_resolver.trusted_ingress_addresses)
    }

    async fn start_api_gateway_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
        tracer: Option<SdkTracer>,
    ) -> Result<(u16, Arc<BoxEndpoint<'static>>), anyhow::Error> {
        let public_endpoint = PublicEndpoint::new(
            self.trusted_ingress(),
            self.services.route_resolver.clone(),
            self.services.mcp_capability_lookup.clone(),
            Route::new()
                .nest("/", custom_api::make_custom_api_endpoint(&self.services))
                .boxed(),
            make_mcp_endpoint(&self.services),
        );
        let route: Arc<BoxEndpoint<'static>> = Arc::new(
            public_endpoint
                .with(OpenTelemetryMetrics::new())
                .with_if_lazy(tracer.is_some(), || {
                    OpenTelemetryTracing::new(tracer.unwrap())
                })
                .boxed(),
        );
        let served_route = route.clone();

        let acceptor = golem_service_base::net::bind_poem_acceptor(
            self.config.socket_addr(self.config.custom_request_port),
        )?;
        let port = acceptor.local_addr()[0]
            .as_socket_addr()
            .expect("socket address")
            .port();

        join_set.spawn(
            async move {
                gateway_server::run(acceptor, served_route)
                    .await
                    .map_err(|err| anyhow!(err).context("API Gateway server failed"))
            }
            .in_current_span(),
        );

        Ok((port, route))
    }

    async fn start_mcp_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
        tracer: Option<SdkTracer>,
    ) -> anyhow::Result<u16> {
        let acceptor = golem_service_base::net::bind_poem_acceptor(
            self.config.socket_addr(self.config.mcp_port),
        )?;

        let port = acceptor.local_addr()[0]
            .as_socket_addr()
            .expect("socket address")
            .port();

        let route = McpOnlyEndpoint::new(
            self.trusted_ingress(),
            self.services.mcp_capability_lookup.clone(),
            make_mcp_endpoint(&self.services),
        )
        .with(OpenTelemetryMetrics::new())
        .with_if_lazy(tracer.is_some(), || {
            OpenTelemetryTracing::new(tracer.unwrap())
        });

        join_set.spawn(
            async move {
                poem::Server::new_with_acceptor(acceptor)
                    .run(route)
                    .await
                    .map_err(|err| anyhow!(err).context("MCP server gateway failed"))
            }
            .in_current_span(),
        );

        Ok(port)
    }
}

#[cfg(test)]
mod mcp_transport_tests {
    use crate::mcp::mcp_http_transport_config;
    use poem::endpoint::TowerCompatExt;
    use poem::{
        Body, Endpoint, Request,
        http::{Method, StatusCode},
    };
    use rmcp::transport::StreamableHttpService;
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use serde_json::json;
    use test_r::test;

    #[derive(Clone)]
    struct Server;
    impl rmcp::ServerHandler for Server {}

    #[test]
    #[test_r::timeout("10s")]
    async fn mcp_export_transport_accepts_deployment_domain_hosts() {
        let endpoint = StreamableHttpService::new(
            || Ok(Server),
            LocalSessionManager::default().into(),
            mcp_http_transport_config(&Default::default()),
        )
        .compat();
        let response = endpoint
            .call(
                Request::builder()
                    .method(Method::POST)
                    .uri("https://customer.example/mcp".parse().unwrap())
                    .header("Host", "customer.example")
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream")
                    .body(
                        Body::from_json(json!({
                            "jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
                                "protocolVersion":"2025-03-26", "capabilities":{},
                                "clientInfo":{"name":"test", "version":"1"}
                            }
                        }))
                        .unwrap(),
                    ),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn idle_mcp_stream(keep_alive: bool) -> (bool, Vec<u8>) {
        use crate::config::McpConfig;
        use crate::transport_test_support::{INTERVAL, SILENCE, idle_proxy, server};
        use poem::EndpointExt;

        let mut config = mcp_http_transport_config(&McpConfig {
            sse_keep_alive_interval: INTERVAL,
        });
        if !keep_alive {
            config = config.with_sse_keep_alive(None);
        }
        let endpoint = StreamableHttpService::new(
            || Ok(Server),
            LocalSessionManager::default().into(),
            config,
        )
        .compat()
        .map_to_response();
        let (address, _server) = server(endpoint).await;
        let (proxy, _proxy) = idle_proxy(address).await;
        let url = format!("http://{proxy}/mcp");
        let client = reqwest::Client::new();
        let initialized = client
            .post(&url)
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                    "protocolVersion": "2025-03-26", "capabilities": {},
                    "clientInfo": {"name": "keep-alive-test", "version": "1"}
                }
            }))
            .send()
            .await
            .unwrap();
        assert!(initialized.status().is_success());
        let session = initialized.headers()["Mcp-Session-Id"].clone();
        initialized.bytes().await.unwrap();
        let notified = client
            .post(&url)
            .header("Accept", "application/json, text/event-stream")
            .header("Mcp-Session-Id", session.clone())
            .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .send()
            .await
            .unwrap();
        assert!(notified.status().is_success());
        let mut response = client
            .get(&url)
            .header("Accept", "text/event-stream")
            .header("Mcp-Session-Id", session)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let mut bytes = Vec::new();
        let closed = tokio::time::timeout(SILENCE, async {
            while let Ok(Some(chunk)) = response.chunk().await {
                bytes.extend_from_slice(&chunk);
            }
        })
        .await
        .is_ok();
        (closed, bytes)
    }

    #[test]
    #[test_r::timeout("15s")]
    async fn mcp_sse_without_keep_alive_loses_proxy_connection() {
        let (closed, _) = idle_mcp_stream(false).await;
        assert!(closed, "the proxy must close an idle SSE connection");
        eprintln!("without MCP SSE keep-alive: stream closed by the 1s-idle proxy");
    }

    #[test]
    #[test_r::timeout("15s")]
    async fn mcp_sse_keep_alive_survives_idle_proxy() {
        let (closed, bytes) = idle_mcp_stream(true).await;
        assert!(!closed, "the SSE stream must stay open");
        assert!(
            bytes.windows(3).any(|bytes| bytes == b":\n\n"),
            "expected RMCP keep-alive comments, got {bytes:?}"
        );
        eprintln!("with MCP SSE keep-alive: stream stayed open for 3s through a 1s-idle proxy");
    }
}
