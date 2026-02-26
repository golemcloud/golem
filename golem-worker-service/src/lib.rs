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

pub mod api;
pub mod bootstrap;
pub mod config;
pub mod custom_api;
pub mod grpcapi;
pub mod mcp;
pub mod metrics;
pub mod model;
pub mod path;
pub mod service;

use crate::bootstrap::Services;
use crate::config::WorkerServiceConfig;
use crate::mcp::GolemAgentMcpServer;
use anyhow::{Context, anyhow};
use golem_common::poem::LazyEndpointExt;
use opentelemetry_sdk::trace::SdkTracer;
use poem::endpoint::TowerCompatExt;
use poem::endpoint::{BoxEndpoint, PrometheusExporter};
use poem::listener::Acceptor;
use poem::listener::Listener;
use poem::middleware::{CookieJarManager, Cors, OpenTelemetryMetrics, OpenTelemetryTracing};
use poem::{EndpointExt, Route};
use prometheus::Registry;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::task::JoinSet;
use tracing::{Instrument, info};

#[cfg(test)]
test_r::enable!();

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
        let grpc_port = self.start_grpc_server(join_set).await?;
        let http_port = self.start_http_server(join_set, tracer.clone()).await?;
        let custom_request_port = self
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
        let grpc_port = self.start_grpc_server(join_set).await?;
        let custom_request_port = self
            .start_api_gateway_server(join_set, tracer.clone())
            .await?;
        let mcp_port = self.start_mcp_server(join_set, tracer).await?;
        let api_endpoint = api::make_open_api_service(&self.services).boxed();

        Ok(TrafficReadyEndpoints {
            grpc_port,
            api_endpoint,
            mcp_port,
            custom_request_port,
        })
    }

    async fn start_grpc_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
    ) -> Result<u16, anyhow::Error> {
        grpcapi::start_grpc_server(&self.config.grpc, self.services.clone(), join_set)
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
            .with_if_lazy(tracer.is_some(), || {
                OpenTelemetryTracing::new(tracer.unwrap())
            });

        let poem_listener =
            poem::listener::TcpListener::bind(format!("0.0.0.0:{}", self.config.port));

        let acceptor = poem_listener.into_acceptor().await?;
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

    async fn start_api_gateway_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
        tracer: Option<SdkTracer>,
    ) -> Result<u16, anyhow::Error> {
        let route = Route::new()
            .nest("/", custom_api::make_custom_api_endpoint(&self.services))
            .with(OpenTelemetryMetrics::new())
            .with_if_lazy(tracer.is_some(), || {
                OpenTelemetryTracing::new(tracer.unwrap())
            });

        let poem_listener = poem::listener::TcpListener::bind(format!(
            "0.0.0.0:{}",
            self.config.custom_request_port
        ));
        let acceptor = poem_listener.into_acceptor().await?;
        let port = acceptor.local_addr()[0]
            .as_socket_addr()
            .expect("socket address")
            .port();

        join_set.spawn(
            async move {
                poem::Server::new_with_acceptor(acceptor)
                    .run(route)
                    .await
                    .map_err(|err| anyhow!(err).context("API Gateway server failed"))
            }
            .in_current_span(),
        );

        Ok(port)
    }

    async fn start_mcp_server(
        &self,
        join_set: &mut JoinSet<anyhow::Result<()>>,
        tracer: Option<SdkTracer>,
    ) -> anyhow::Result<u16> {
        let poem_listener = poem::listener::TcpListener::bind(format!(
            "0.0.0.0:{}",
            self.config.mcp_port
        ));

        let acceptor = poem_listener.into_acceptor().await?;

        let port = acceptor.local_addr()[0]
            .as_socket_addr()
            .expect("socket address")
            .port();

        let mcp_capability_lookup =
            self.services.mcp_capability_lookup.clone();

        let worker_service =
            self.services.worker_service.clone();

        let service = StreamableHttpService::new(
            move || {
                Ok(GolemAgentMcpServer::new(
                    mcp_capability_lookup.clone(),
                    worker_service.clone(),
                ))
            },
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default(),
        );

        let route = Route::new()
            .nest("/mcp", service.compat())
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
