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

use crate::StartedComponents;
use anyhow::Context;
use poem::middleware::{CookieJarManager, Cors, OpenTelemetryMetrics, Tracing};
use poem::EndpointExt;
use poem::{Route, Server};
use std::net::Ipv4Addr;
use tokio::task::JoinSet;
use tracing::info;
use tracing::Instrument;

pub fn start_router(
    listener_addr: &str,
    listener_port: u16,
    started_components: StartedComponents,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    use std::net::SocketAddrV4;
    use std::sync::Arc;

    use poem::endpoint::PrometheusExporter;
    use poem::listener::TcpListener;

    info!("Starting single-executable http api");

    let ipv4_addr: Ipv4Addr = listener_addr.parse().context(format!(
        "Failed at parsing the listener host address {listener_addr}"
    ))?;

    let listener_socket_addr = SocketAddrV4::new(ipv4_addr, listener_port);

    let listener = TcpListener::bind(listener_socket_addr);

    let metrics = PrometheusExporter::new(started_components.prometheus_registry.clone());

    let worker_service_api = Arc::new(started_components.worker_service.api_endpoint);
    let component_service_api = Arc::new(started_components.component_service.endpoint);
    let cloud_service_api = Arc::new(started_components.cloud_service.endpoint);

    let app = Route::new()
        .at("/v1/api/definitions", worker_service_api.clone())
        .at("/v1/api/definitions/*", worker_service_api.clone())
        .at("/v1/api/deployments", worker_service_api.clone())
        .at("/v1/api/deployments/*", worker_service_api.clone())
        .at("/v1/api/security", worker_service_api.clone())
        .at("/v1/api/security/*", worker_service_api.clone())
        .at("/v1/app-plugins", component_service_api.clone())
        .at("/v1/components", component_service_api.clone())
        .at(
            "/v1/components/:component_id",
            component_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/download",
            component_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/invoke",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/invoke-and-await",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/latest",
            component_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/latest/*",
            component_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/updates",
            component_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/upload",
            component_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/versions",
            component_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/versions/*",
            component_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/*",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:worker_id/connect",
            worker_service_api.clone(),
        )
        .at("/v1/library-plugins", component_service_api.clone())
        .at("/v1/plugins", component_service_api.clone())
        .at("/v1/plugins/*", component_service_api.clone())
        .at("/v1/accounts", cloud_service_api.clone())
        .at("/v1/accounts/*", cloud_service_api.clone())
        .at("/v1/admin/*", cloud_service_api.clone())
        .at("/v1/resource-limits", cloud_service_api.clone())
        .at("/v1/oauth2", cloud_service_api.clone())
        .at("/v1/login/*", cloud_service_api.clone())
        .at("/v1/projects", cloud_service_api.clone())
        .at("/v1/projects/*", cloud_service_api.clone())
        .at("/metrics", metrics)
        .at("/healthcheck", component_service_api)
        .with(CookieJarManager::new())
        .with(Cors::new().allow_origin_regex(".*").allow_credentials(true))
        .with(OpenTelemetryMetrics::new())
        .with(Tracing);

    // TODO: have proper healtchchecks, pass them through the different services and expose here -- similar to metrics
    // for now just use the one from component service.

    join_set.spawn(
        async move { Server::new(listener).run(app).await.map_err(|e| e.into()) }.in_current_span(),
    );

    Ok(())
}
