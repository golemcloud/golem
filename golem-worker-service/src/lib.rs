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
