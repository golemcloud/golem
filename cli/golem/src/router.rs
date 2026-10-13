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
use golem_common::base_model::api;
use golem_common::poem::{CliClientInfoMiddleware, OpenTelemetryMetrics, PrometheusExporter};
use golem_registry_service::startup::{StartupPhase, StartupProgress};
use poem::endpoint::BoxEndpoint;
use poem::http::{StatusCode, header};
use poem::listener::Acceptor;
use poem::middleware::{CookieJarManager, Cors, Tracing};
use poem::{Endpoint, EndpointExt, Request, Response, Route, Server};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use tokio::task::JoinSet;
use tracing::Instrument;
use tracing::info;

/// The single-executable HTTP API, bound before the services start.
///
/// Until [`StartingRouter::serve`] is called, every request (including `/healthcheck`) is answered
/// with `503 Service Unavailable` describing the current [`StartupPhase`], so clients can tell a
/// starting server apart from one that is not running.
pub struct StartingRouter {
    port: u16,
    ready: Arc<OnceLock<ReadyRouter>>,
}

struct ReadyRouter {
    app: BoxEndpoint<'static, Response>,
    // Keeps the worker executor alive for as long as the router is serving requests. Dropping its
    // RunDetails cancels the shutdown token that drives background services such as the scheduler
    // loop.
    _worker_executor: golem_worker_executor::RunDetails,
}

struct StartupGate {
    ready: Arc<OnceLock<ReadyRouter>>,
    startup_progress: StartupProgress,
}

impl Endpoint for StartupGate {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Response> {
        match self.ready.get() {
            Some(router) => router.app.call(req).await,
            None => Ok(server_starting_response(&self.startup_progress.current())),
        }
    }
}

fn server_starting_response(phase: &StartupPhase) -> Response {
    let body = serde_json::json!({
        "code": api::error_code::SERVER_STARTING,
        "error": format!("The Golem server is still starting: {phase}"),
    });
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::RETRY_AFTER, "1")
        .header(
            api::header::GOLEM_SERVER_STATUS,
            api::header::GOLEM_SERVER_STATUS_STARTING,
        )
        .content_type("application/json")
        .body(body.to_string())
}

pub async fn bind_router(
    listener_addr: &str,
    listener_port: u16,
    startup_progress: StartupProgress,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<StartingRouter, anyhow::Error> {
    let ip_addr: IpAddr = listener_addr.parse().context(format!(
        "Failed at parsing the listener host address {listener_addr}"
    ))?;

    let acceptor =
        golem_service_base::net::bind_poem_acceptor(SocketAddr::new(ip_addr, listener_port))?;
    let port = acceptor.local_addr()[0]
        .as_socket_addr()
        .expect("socket address")
        .port();

    let ready = Arc::new(OnceLock::new());
    let gate = StartupGate {
        ready: ready.clone(),
        startup_progress,
    };

    join_set.spawn(
        async move {
            Server::new_with_acceptor(acceptor)
                .run(gate)
                .await
                .map_err(|e| e.into())
        }
        .in_current_span(),
    );

    Ok(StartingRouter { port, ready })
}

impl StartingRouter {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn serve(self, started_components: StartedComponents) {
        let StartedComponents {
            registry_service,
            worker_executor,
            worker_service,
            prometheus_registry,
            ..
        } = started_components;

        info!("Starting single-executable http api");

        let metrics = PrometheusExporter::new(prometheus_registry.clone());

        let worker_service_api = Arc::new(worker_service.api_endpoint);
        let registry_service_api = Arc::new(registry_service.endpoint);

        let app = Route::new()
            // Worker endpoints
            .at("/v1/agents/create-agent", worker_service_api.clone())
            .at("/v1/agents/invoke-agent", worker_service_api.clone())
            .at("/v1/agents/invoke-tool", worker_service_api.clone())
            .at("/v1/agents/describe-tool", worker_service_api.clone())
            .at(
                "/v1/agents/invoke-agent-session",
                worker_service_api.clone(),
            )
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
            .at(
                "/v1/components/:component_id/workers/:agent_name/wallet",
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

        // `/healthcheck` is intentionally served by the registry service's static endpoint (via the `*`
        // fallback above) rather than an aggregated per-service check. Until every service has started
        // and bound its ports, the startup gate answers it with 503, so this is a "server process is up"
        // signal for the local tooling that polls it. It deliberately does NOT reflect deep readiness:
        // worker executors register with an empty shard set and receive assignments in the background,
        // the shard manager reconciles its ring in the background, and the worker service connects to
        // the registry lazily. Since no service gates its own health on that readiness, an aggregated
        // check here could not report it either. Deployed servers use each service's own per-service
        // probe, so aggregating here would add nothing.

        if self
            .ready
            .set(ReadyRouter {
                app: app.map_to_response().boxed(),
                _worker_executor: worker_executor,
            })
            .is_err()
        {
            unreachable!("StartingRouter::serve consumes the router, so it runs only once");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_registry_service::startup::BuiltinArtifactKind;
    use poem::test::TestClient;
    use test_r::test;

    #[test]
    async fn requests_are_answered_with_the_startup_phase_until_the_router_is_ready() {
        let startup_progress = StartupProgress::default();
        let client = TestClient::new(StartupGate {
            ready: Arc::new(OnceLock::new()),
            startup_progress: startup_progress.clone(),
        });

        startup_progress.set_phase(StartupPhase::FetchingBuiltinArtifacts {
            kind: BuiltinArtifactKind::Tools,
            completed: 0,
            total: 6,
            downloaded_bytes: 0,
        });
        startup_progress.step_completed();

        for path in ["/healthcheck", "/v1/me"] {
            let response = client.get(path).send().await;
            response.assert_status(StatusCode::SERVICE_UNAVAILABLE);
            response.assert_header(
                api::header::GOLEM_SERVER_STATUS,
                api::header::GOLEM_SERVER_STATUS_STARTING,
            );
            response.assert_header(header::RETRY_AFTER, "1");
            response
                .assert_json(serde_json::json!({
                    "code": api::error_code::SERVER_STARTING,
                    "error": "The Golem server is still starting: fetching built-in tools (1/6 ready)",
                }))
                .await;
        }
    }
}
