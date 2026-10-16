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
use golem_common::model::domain_registration::Domain;
use golem_common::poem::{CliClientInfoMiddleware, OpenTelemetryMetrics, PrometheusExporter};
use golem_registry_service::startup::{StartupPhase, StartupProgress};
use golem_worker_service::ingress::TrustedIngress;
use poem::endpoint::BoxEndpoint;
use poem::http::{StatusCode, header};
use poem::listener::Acceptor;
use poem::middleware::{CookieJarManager, Cors, Tracing};
use poem::{Endpoint, EndpointExt, Request, Response, Route};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use tokio::task::JoinSet;
use tracing::Instrument;
use tracing::{debug, info};

/// The single-executable HTTP API, bound before the services start.
///
/// Until [`StartingRouter::serve`] is called, every request (including `/healthcheck`) is answered
/// with `503 Service Unavailable` describing the current [`StartupPhase`], so clients can tell a
/// starting server apart from one that is not running.
///
/// Without a management host, every request is a management API request. With a management
/// host, only requests for that host reach the management API; requests for every other host
/// reach the worker service's public endpoint, which serves HTTP API and MCP deployments.
pub struct StartingRouter {
    port: u16,
    management_host: Option<String>,
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
    management_host: Option<&str>,
    startup_progress: StartupProgress,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<StartingRouter, anyhow::Error> {
    let ip_addr: IpAddr = listener_addr.parse().context(format!(
        "Failed at parsing the listener host address {listener_addr}"
    ))?;
    let management_host = management_host
        .map(|host| Domain::parse_host(host).map_err(anyhow::Error::msg))
        .transpose()
        .context("Invalid router management host")?;

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
            golem_worker_service::gateway_server::run(acceptor, gate)
                .await
                .map_err(|e| e.into())
        }
        .in_current_span(),
    );

    Ok(StartingRouter {
        port,
        management_host,
        ready,
    })
}

impl StartingRouter {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn serve(self, started_components: StartedComponents, cors_origin_regex: &str) {
        let StartedComponents {
            registry_service,
            worker_executor,
            worker_service,
            prometheus_registry,
            #[cfg(feature = "local-observability")]
            observability,
            ..
        } = started_components;

        info!("Starting single-executable http api");

        let metrics = PrometheusExporter::new(prometheus_registry.clone());

        let worker_service_api = Arc::new(worker_service.api_endpoint);
        let registry_service_api = Arc::new(registry_service.endpoint);

        let management = Route::new()
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
                "/v1/components/:component_id/workers/:agent_name/oplog/search",
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
            .at("*", registry_service_api.clone());
        #[cfg(feature = "local-observability")]
        let management = match observability {
            Some(observability) => crate::observability::add_routes(
                management,
                observability,
                golem_client::LOCAL_WELL_KNOWN_TOKEN,
            ),
            None => management,
        };
        let management = with_management_middleware(management, cors_origin_regex);

        let app = match self.management_host {
            None => management,
            Some(management_host) => HostRouter {
                management_host,
                trusted_ingress: worker_service.trusted_ingress,
                management,
                public: worker_service.public_endpoint,
            }
            .boxed(),
        };

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
                app,
                _worker_executor: worker_executor,
            })
            .is_err()
        {
            unreachable!("StartingRouter::serve consumes the router, so it runs only once");
        }
    }
}

/// Applies the middleware of the management API. Requests for the public endpoint do not pass
/// through it.
fn with_management_middleware(
    management: impl Endpoint + 'static,
    cors_origin_regex: &str,
) -> BoxEndpoint<'static> {
    management
        .with(CookieJarManager::new())
        .with(
            Cors::new()
                .allow_origin_regex(cors_origin_regex)
                .allow_credentials(true),
        )
        .with(CliClientInfoMiddleware::new())
        .with(OpenTelemetryMetrics::new())
        .with(Tracing)
        .boxed()
}

/// Sends requests for the management host to the management API, and every other request to
/// the public endpoint. Hosts are compared without case, port and trailing dot.
struct HostRouter {
    management_host: String,
    trusted_ingress: TrustedIngress,
    management: BoxEndpoint<'static>,
    public: BoxEndpoint<'static>,
}

impl Endpoint for HostRouter {
    type Output = Response;

    async fn call(&self, req: Request) -> poem::Result<Self::Output> {
        let is_management = match self.trusted_ingress.public_origin(&req) {
            Ok(origin) => Domain(origin.authority).host() == self.management_host,
            Err(_) => {
                debug!("Routing a request with an invalid origin to the public endpoint");
                false
            }
        };
        if is_management {
            self.management.call(req).await
        } else {
            self.public.call(req).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_registry_service::startup::BuiltinArtifactKind;
    use poem::test::TestClient;
    use poem::{Body, http};
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

    fn named(name: &'static str) -> BoxEndpoint<'static> {
        poem::endpoint::make_sync(move |_: Request| name)
            .map_to_response()
            .boxed()
    }

    fn router(trusted: &[&str]) -> HostRouter {
        router_for("api.golem.example.com", trusted)
    }

    fn router_for(management_host: &str, trusted: &[&str]) -> HostRouter {
        HostRouter {
            management_host: Domain::parse_host(management_host).unwrap(),
            trusted_ingress: TrustedIngress::new(
                &trusted
                    .iter()
                    .map(|address| address.parse().unwrap())
                    .collect::<Vec<_>>(),
            ),
            management: named("management"),
            public: named("public"),
        }
    }

    fn request(peer: &str, headers: &[(&str, &str)]) -> Request {
        request_with(
            peer,
            http::Request::builder().uri("/v1/components"),
            headers,
        )
    }

    fn request_with(
        peer: &str,
        mut builder: http::request::Builder,
        headers: &[(&str, &str)],
    ) -> Request {
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let (parts, _) = builder.body(()).unwrap().into_parts();
        let parts = poem::RequestParts::from((
            parts,
            poem::web::LocalAddr(poem::Addr::socket(([127, 0, 0, 1], 9881).into())),
            poem::web::RemoteAddr(poem::Addr::SocketAddr(peer.parse().unwrap())),
            http::uri::Scheme::HTTP,
        ));
        Request::from_parts(parts, Body::empty())
    }

    async fn served_by(router: &HostRouter, req: Request) -> String {
        let response = router.call(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.into_body().into_string().await.unwrap()
    }

    #[test]
    async fn management_host_reaches_the_management_api() {
        let router = router(&[]);
        for host in [
            "api.golem.example.com",
            "API.Golem.Example.com",
            "api.golem.example.com:443",
            "api.golem.example.com.",
        ] {
            assert_eq!(
                served_by(&router, request("10.0.0.2:5000", &[("Host", host)])).await,
                "management",
                "{host}"
            );
        }
    }

    #[test]
    async fn other_hosts_reach_the_public_endpoint_even_on_management_paths() {
        let router = router(&[]);
        for host in [
            "shop.golem.example.com",
            "api.golem.example.com.evil.test",
            "golem.example.com",
        ] {
            assert_eq!(
                served_by(&router, request("10.0.0.2:5000", &[("Host", host)])).await,
                "public",
                "{host}"
            );
        }
        assert_eq!(
            served_by(&router, request("10.0.0.2:5000", &[])).await,
            "public"
        );
    }

    #[test]
    async fn only_a_trusted_ingress_can_select_the_host_with_forwarded_headers() {
        let router = router(&["10.0.0.1"]);
        let headers = [
            ("Host", "golem:9881"),
            ("X-Forwarded-Host", "api.golem.example.com"),
            ("X-Forwarded-Proto", "https"),
        ];
        assert_eq!(
            served_by(&router, request("10.0.0.1:5000", &headers)).await,
            "management"
        );
        assert_eq!(
            served_by(&router, request("10.0.0.2:5000", &headers)).await,
            "public"
        );
    }

    #[test]
    async fn trusted_ingress_without_forwarded_headers_reaches_the_public_endpoint() {
        let router = router(&["10.0.0.1"]);
        assert_eq!(
            served_by(
                &router,
                request("10.0.0.1:5000", &[("Host", "api.golem.example.com")])
            )
            .await,
            "public"
        );
    }

    #[test]
    async fn http2_request_is_routed_by_its_authority() {
        let router = router(&[]);
        let builder = http::Request::builder()
            .version(http::Version::HTTP_2)
            .uri("http://api.golem.example.com/v1/components");
        assert_eq!(
            served_by(&router, request_with("10.0.0.2:5000", builder, &[])).await,
            "management"
        );
    }

    #[test]
    async fn ipv6_management_host_is_compared_in_canonical_form() {
        let router = router_for("[0:0::1]:9881", &[]);
        assert_eq!(
            served_by(&router, request("10.0.0.2:5000", &[("Host", "[::1]:9881")])).await,
            "management"
        );
    }

    #[test]
    async fn management_cors_does_not_apply_to_public_traffic() {
        let router = HostRouter {
            management: with_management_middleware(
                poem::endpoint::make_sync(|_: Request| "management"),
                ".*",
            ),
            ..router(&[])
        };
        let preflight = |host: &'static str| {
            request(
                "10.0.0.2:5000",
                &[
                    ("Host", host),
                    ("Origin", "https://shop.golem.example.com"),
                    ("Access-Control-Request-Method", "GET"),
                ],
            )
        };

        // The management CORS policy rejects this origin.
        assert!(
            router
                .call(preflight("api.golem.example.com"))
                .await
                .is_err()
        );

        let response = router
            .call(preflight("shop.golem.example.com"))
            .await
            .unwrap();
        assert_eq!(response.into_body().into_string().await.unwrap(), "public");
    }
}
