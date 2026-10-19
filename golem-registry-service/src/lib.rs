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
#[cfg(any(not(feature = "embed-builtin-artifacts"), test))]
mod builtin_manifest;
pub mod config;
pub mod grpc;
pub mod metrics;
pub mod model;
pub mod repo;
pub mod services;
pub mod startup;

use self::bootstrap::Services;
use self::config::RegistryServiceConfig;
use self::startup::StartupProgress;
use anyhow::Context;
use golem_common::poem::{
    CliClientInfoMiddleware, LazyEndpointExt, OpenTelemetryTracing, PrometheusExporter,
};
use golem_service_base::body_limit::RequestBodyLimit;
use golem_service_base::http_server;
use opentelemetry_sdk::trace::SdkTracer;
use poem::endpoint::BoxEndpoint;
use poem::listener::Acceptor;
use poem::middleware::CookieJarManager;
use poem::middleware::Cors;
use poem::{EndpointExt, Route};
use tokio::task::JoinSet;
use tracing::{Instrument, info};

#[cfg(test)]
test_r::enable!();

pub struct RunDetails {
    pub grpc_port: u16,
    pub http_port: u16,
}

pub struct SingleExecutableRunDetails {
    pub grpc_port: u16,
    pub endpoint: BoxEndpoint<'static>,
}

#[derive(Clone)]
pub struct RegistryService {
    config: RegistryServiceConfig,
    prometheus_registry: prometheus::Registry,
    startup_progress: StartupProgress,
}

impl RegistryService {
    pub fn new(config: RegistryServiceConfig, prometheus_registry: prometheus::Registry) -> Self {
        Self {
            config,
            prometheus_registry,
            startup_progress: StartupProgress::default(),
        }
    }

    pub fn with_startup_progress(mut self, startup_progress: StartupProgress) -> Self {
        self.startup_progress = startup_progress;
        self
    }

    pub async fn start(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
        tracer: Option<SdkTracer>,
    ) -> Result<RunDetails, anyhow::Error> {
        info!("Initializing registry service");
        let services = Services::new(&self.config, join_set, &self.startup_progress).await?;

        let http_port = self.start_http_server(join_set, tracer, &services).await?;
        let grpc_port = self.start_grpc_server(join_set, &services).await?;

        Ok(RunDetails {
            http_port,
            grpc_port,
        })
    }

    /// Endpoints are only valid until joinset is dropped
    pub async fn start_for_single_executable(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
    ) -> Result<SingleExecutableRunDetails, anyhow::Error> {
        info!("Initializing registry service");
        let services = Services::new(&self.config, join_set, &self.startup_progress).await?;

        let grpc_port = self.start_grpc_server(join_set, &services).await?;
        let endpoint = api::make_open_api_service(&services)
            .with(self.body_limit())
            .boxed();

        Ok(SingleExecutableRunDetails {
            grpc_port,
            endpoint,
        })
    }

    fn body_limit(&self) -> RequestBodyLimit {
        let limits = &self.config.body_limits;
        RequestBodyLimit::new(limits.api).with_multipart_limit(limits.upload)
    }

    async fn start_grpc_server(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
        services: &Services,
    ) -> Result<u16, anyhow::Error> {
        let port = crate::grpc::start_grpc_server(
            self.config.grpc_addr(),
            &self.config.grpc,
            services,
            join_set,
        )
        .await
        .context("starting gRPC server failed")?;

        info!("Started registry-service grpc server on port {port}");
        Ok(port)
    }

    async fn start_http_server(
        &self,
        join_set: &mut JoinSet<Result<(), anyhow::Error>>,
        tracer: Option<SdkTracer>,
        services: &Services,
    ) -> Result<u16, anyhow::Error> {
        let prometheus_registry = self.prometheus_registry.clone();

        let api_service = api::make_open_api_service(services);

        let ui = api_service.swagger_ui();
        let spec = api_service.spec_endpoint_yaml();
        let metrics = PrometheusExporter::new(prometheus_registry.clone());

        let cors = Cors::new()
            .allow_origin_regex(&self.config.cors_origin_regex)
            .allow_credentials(true);

        let app = Route::new()
            .nest("/", api_service)
            .nest("/docs", ui)
            .nest("/specs", spec)
            .nest("/metrics", metrics)
            .with(self.body_limit())
            .with(CookieJarManager::new())
            .with(cors)
            .with(CliClientInfoMiddleware::new())
            .with_if_lazy(tracer.is_some(), || {
                OpenTelemetryTracing::new(tracer.unwrap())
            });

        let acceptor = golem_service_base::net::bind_poem_acceptor(self.config.http_addr())?;
        let port = acceptor.local_addr()[0]
            .as_socket_addr()
            .expect("socket address")
            .port();

        let http_server_config = self.config.http_server.clone();
        join_set.spawn(
            async move {
                http_server::run(acceptor, app, http_server_config)
                    .await
                    .map_err(|e| e.into())
            }
            .in_current_span(),
        );

        info!("Started registry-service http server on port {port}");

        Ok(port)
    }
}
