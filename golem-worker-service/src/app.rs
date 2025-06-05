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

use crate::config::WorkerServiceCloudConfig;
use crate::service::ApiServices;
use crate::{api, grpcapi};
use opentelemetry::global;
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use poem::endpoint::PrometheusExporter;
use poem::middleware::{CookieJarManager, Cors, OpenTelemetryMetrics, Tracing};
use poem::{EndpointExt, Route};
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::select;

pub async fn dump_openapi_yaml() -> Result<String, String> {
    let config = WorkerServiceCloudConfig::default();
    let services = ApiServices::new(&config).await?;
    Ok(api::make_open_api_service(services).spec_yaml())
}

pub async fn app(config: &WorkerServiceCloudConfig) -> std::io::Result<()> {
    let prometheus_registry = prometheus::Registry::new();

    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(prometheus_registry.clone())
        .build()
        .unwrap();

    global::set_meter_provider(
        MeterProviderBuilder::default()
            .with_reader(exporter)
            .build(),
    );

    let services: ApiServices = ApiServices::new(config)
        .await
        .map_err(std::io::Error::other)?;

    let cloud_specific_config = config.cloud_specific_config.clone();
    let config = config.base_config.clone();

    let http_service1 = services.clone();
    let http_service2 = services.clone();
    let grpc_services = services.clone();

    let custom_request_server = tokio::spawn(async move {
        let route = api::custom_http_request_route(http_service1)
            .with(OpenTelemetryMetrics::new())
            .with(Tracing);

        poem::Server::new(poem::listener::TcpListener::bind((
            "0.0.0.0",
            config.custom_request_port,
        )))
        .name("custom_request")
        .run(route)
        .await
        .expect("Custom Request server failed")
    });

    let worker_server = tokio::spawn(async move {
        let cors = Cors::new()
            .allow_origin_regex(&cloud_specific_config.cors_origin_regex)
            .allow_credentials(true);

        let app = Route::new()
            .nest("/", api::management_routes(http_service2))
            .nest("/metrics", PrometheusExporter::new(prometheus_registry))
            .with(CookieJarManager::new())
            .with(cors);

        poem::Server::new(poem::listener::TcpListener::bind(("0.0.0.0", config.port)))
            .name("api")
            .run(app)
            .await
            .expect("HTTP server failed");
    });

    let grpc_server = tokio::spawn(async move {
        grpcapi::start_grpc_server(
            SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), config.worker_grpc_port).into(),
            grpc_services,
        )
        .await
        .expect("gRPC server failed");
    });

    select! {
        _ = worker_server => {},
        _ = custom_request_server => {},
        _ = grpc_server => {},
    }
    Ok(())
}
