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

use crate::StartedComponents;
use anyhow::Context;
use golem_common::poem::CliClientInfoMiddleware;
use poem::EndpointExt;
use poem::listener::{Acceptor, Listener};
use poem::middleware::{CookieJarManager, Cors, OpenTelemetryMetrics, Tracing};
use poem::{Route, Server};
use std::net::Ipv4Addr;
use tokio::task::JoinSet;
use tracing::Instrument;
use tracing::info;

pub async fn start_router(
    listener_addr: &str,
    listener_port: u16,
    started_components: StartedComponents,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<u16, anyhow::Error> {
    use std::net::SocketAddrV4;
    use std::sync::Arc;

    use poem::endpoint::PrometheusExporter;
    use poem::listener::TcpListener;

    let StartedComponents {
        registry_service,
        worker_executor,
        worker_service,
        prometheus_registry,
        ..
    } = started_components;

    info!("Starting single-executable http api");

    let ipv4_addr: Ipv4Addr = listener_addr.parse().context(format!(
        "Failed at parsing the listener host address {listener_addr}"
    ))?;

    let listener_socket_addr = SocketAddrV4::new(ipv4_addr, listener_port);

    let listener = TcpListener::bind(listener_socket_addr);
    let acceptor = listener.into_acceptor().await?;
    let port = acceptor.local_addr()[0]
        .as_socket_addr()
        .expect("socket address")
        .port();

    let metrics = PrometheusExporter::new(prometheus_registry.clone());

    let worker_service_api = Arc::new(worker_service.api_endpoint);
    let registry_service_api = Arc::new(registry_service.endpoint);

    let app = Route::new()
        // Worker endpoints
        .at("/v1/agents/create-agent", worker_service_api.clone())
        .at("/v1/agents/invoke-agent", worker_service_api.clone())
        .at(
            "/v1/components/:component_id/workers",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/find",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/activate-plugin",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/complete",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/connect",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/deactivate-plugin",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/file-contents/:file_name",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/files/:file_name",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/fork",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/interrupt",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/invocations/:idempotency_key",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/invoke",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/invoke-and-await",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/oplog",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/resume",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/revert",
            worker_service_api.clone(),
        )
        .at(
            "/v1/components/:component_id/workers/:agent_name/update",
            worker_service_api.clone(),
        )
        // Metrics
        .at("/metrics", metrics)
        // Everything else is routed to registry service
        .at("*", registry_service_api.clone())
        .with(CookieJarManager::new())
        .with(Cors::new().allow_origin_regex(".*").allow_credentials(true))
        .with(CliClientInfoMiddleware::new())
        .with(OpenTelemetryMetrics::new())
        .with(Tracing);

    // TODO: have proper healtchchecks, pass them through the different services and expose here -- similar to metrics
    // for now just use the one from component service.

    join_set.spawn(
        async move {
            // Keep the worker executor alive for as long as the router is serving requests.
            // Dropping its RunDetails cancels the shutdown token that drives background services
            // such as the scheduler loop.
            let _worker_executor = worker_executor;

            Server::new_with_acceptor(acceptor)
                .run(app)
                .await
                .map_err(|e| e.into())
        }
        .in_current_span(),
    );

    Ok(port)
}
