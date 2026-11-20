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

use super::auth::no_mcp_deployment_response;
use super::revision::ServedRevisions;
use super::{
    GolemAgentMcpServer, McpBearerAuth, McpCapabilitiesLookupError, McpCapabilityLookup,
    McpDeployment, oauth_proxy_routes,
};
use crate::bootstrap::Services;
use crate::custom_api::oidc::IdentityProvider;
use crate::custom_api::oidc::session_store::SessionStore;
use crate::ingress::{PublicOrigin, TrustedIngress};
use crate::service::worker::WorkerService;
use golem_common::model::domain_registration::Domain;
use golem_service_base::body_limit::RequestBodyLimit;
use poem::endpoint::{BoxEndpoint, TowerCompatExt};
use poem::middleware::Cors;
use poem::{Endpoint, EndpointExt, IntoResponse, Request, Response, Result, http};
use rmcp::ServerHandler;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::Arc;

/// The MCP deployment that serves a request, and the public origin the request arrived at.
///
/// The listener resolves both before the request reaches the MCP endpoint, so the MCP
/// handlers never read the host or the forwarded headers themselves.
#[derive(Clone)]
pub struct McpRequest {
    pub origin: PublicOrigin,
    pub deployment: Arc<McpDeployment>,
}

/// The outcome of looking up the MCP deployment for a request's public origin.
pub enum McpDomainLookup {
    Found(McpRequest),
    NotFound,
    Failed,
}

pub async fn lookup_mcp_domain(
    mcp_capability_lookup: &dyn McpCapabilityLookup,
    origin: PublicOrigin,
) -> McpDomainLookup {
    match mcp_capability_lookup
        .get(&Domain(origin.authority.clone()))
        .await
    {
        Ok(deployment) => McpDomainLookup::Found(McpRequest { origin, deployment }),
        Err(McpCapabilitiesLookupError::UnknownSite(_)) => McpDomainLookup::NotFound,
        Err(McpCapabilitiesLookupError::InternalError(error)) => {
            tracing::warn!(error = %error, "Failed to look up the MCP deployment of a domain");
            McpDomainLookup::Failed
        }
    }
}

/// Builds the MCP endpoint: the OAuth proxy routes, the `/mcp` transport, and their CORS policy.
/// Every request must carry an [`McpRequest`] extension.
pub fn make_mcp_endpoint(services: &Services, max_body_size: usize) -> BoxEndpoint<'static> {
    mcp_endpoint(
        services.mcp_capability_lookup.clone(),
        services.worker_service.clone(),
        services.identity_provider.clone(),
        services.session_store.clone(),
        &services.mcp_config,
        max_body_size,
    )
}

/// The MCP endpoint over rmcp. It keeps no state between requests: it serves each request with
/// the deployment that the request's [`McpRequest`] names.
pub(crate) fn mcp_endpoint(
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
    worker_service: Arc<WorkerService>,
    identity_provider: Arc<dyn IdentityProvider>,
    session_store: Arc<dyn SessionStore>,
    config: &crate::config::McpConfig,
    max_body_size: usize,
) -> BoxEndpoint<'static> {
    mcp_routes(
        stateless_transport(golem_handler(mcp_capability_lookup, worker_service), config),
        identity_provider,
        session_store,
        max_body_size,
    )
}

fn golem_handler(
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
    worker_service: Arc<WorkerService>,
) -> impl Fn() -> GolemAgentMcpServer + Send + Sync + 'static {
    move || GolemAgentMcpServer::new(mcp_capability_lookup.clone(), worker_service.clone())
}

/// The OAuth proxy routes and the `/mcp` `transport` behind the bearer authentication, with their
/// body limit and CORS policy.
fn mcp_routes(
    transport: impl Endpoint<Output = Response> + 'static,
    identity_provider: Arc<dyn IdentityProvider>,
    session_store: Arc<dyn SessionStore>,
    max_body_size: usize,
) -> BoxEndpoint<'static> {
    let oauth_routes = oauth_proxy_routes(identity_provider.clone(), session_store);

    let mcp_with_auth = transport.with(McpBearerAuth::new(identity_provider));

    // OAuth routes use .at() so they take priority over .nest("/mcp", ...)
    oauth_routes
        .nest("/mcp", mcp_with_auth)
        // Inside CORS, so that a 413 response carries the CORS headers.
        .with(RequestBodyLimit::new(max_body_size))
        .with(
            Cors::new()
                .allow_methods(vec!["GET", "POST", "DELETE", "OPTIONS"])
                .allow_headers(vec![
                    "Content-Type",
                    "Authorization",
                    "Accept",
                    "MCP-Protocol-Version",
                ]),
        )
        .map_to_response()
        .boxed()
}

/// The rmcp service for `handler`s behind [`ServedRevisions`]. It serves every request
/// statelessly and has no event store, so `GET` and `DELETE` get `405`. Each request must carry
/// its `Arc<McpDeployment>` and an `McpServing` extension.
fn stateless_transport<S, H>(
    handler: H,
    config: &crate::config::McpConfig,
) -> impl Endpoint<Output = Response> + use<S, H>
where
    S: ServerHandler + Send + 'static,
    H: Fn() -> S + Send + Sync + 'static,
{
    StreamableHttpService::new(
        move || Ok(handler()),
        Arc::new(NeverSessionManager::default()),
        mcp_http_transport_config(config),
    )
    .compat()
    .with(ServedRevisions)
}

pub(crate) fn mcp_http_transport_config(
    config: &crate::config::McpConfig,
) -> StreamableHttpServerConfig {
    // Export hosts are deployment domains, not a static loopback allowlist.
    // Export request limits are independent of the bounded outbound import client.
    // The endpoint keeps no session and resumes no stream, so it sends no SSE retry value.
    StreamableHttpServerConfig::default()
        .with_sse_keep_alive(Some(config.sse_keep_alive_interval))
        .with_sse_retry(None)
        .with_legacy_session_mode(false)
        .with_json_response(false)
        .disable_allowed_hosts()
        .with_max_request_body_bytes(usize::MAX)
}

#[cfg(test)]
mod transport_tests;

/// Serves only MCP deployments: resolves the request's MCP deployment and passes the request
/// to the MCP endpoint. Requests for any other host get a 404.
pub struct McpOnlyEndpoint {
    trusted_ingress: TrustedIngress,
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
    mcp: BoxEndpoint<'static>,
}

impl McpOnlyEndpoint {
    pub fn new(
        trusted_ingress: TrustedIngress,
        mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
        mcp: BoxEndpoint<'static>,
    ) -> Self {
        Self {
            trusted_ingress,
            mcp_capability_lookup,
            mcp,
        }
    }
}

impl Endpoint for McpOnlyEndpoint {
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        let Ok(origin) = self.trusted_ingress.public_origin(&req) else {
            return Ok(Response::builder()
                .status(http::StatusCode::BAD_REQUEST)
                .body("Invalid request origin"));
        };
        match lookup_mcp_domain(self.mcp_capability_lookup.as_ref(), origin).await {
            McpDomainLookup::Found(mcp_request) => {
                req.extensions_mut().insert(mcp_request);
                self.mcp.call(req).await.map(IntoResponse::into_response)
            }
            McpDomainLookup::NotFound => Ok(no_mcp_deployment_response()),
            McpDomainLookup::Failed => Ok(Response::builder()
                .status(http::StatusCode::SERVICE_UNAVAILABLE)
                .finish()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingress::tests::request_from_peer;
    use crate::mcp::invoke::test_support::streamed_messages;
    use futures::StreamExt;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::AgentMode;
    use golem_common::model::application::ApplicationName;
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_service_base::mcp::CompiledMcp;
    use poem::Body;
    use poem::endpoint::make_sync;
    use serde_json::{Value, json};
    use test_r::test;

    pub(super) struct Lookup;

    #[async_trait::async_trait]
    impl McpCapabilityLookup for Lookup {
        async fn get(
            &self,
            domain: &Domain,
        ) -> Result<Arc<McpDeployment>, McpCapabilitiesLookupError> {
            match domain.0.as_str() {
                "tools.example.com" => Ok(Arc::new(McpDeployment::new(CompiledMcp {
                    account_id: AccountId::new(),
                    account_email: AccountEmail::new("owner@example.com"),
                    application_name: ApplicationName("app".into()),
                    environment_id: EnvironmentId::new(),
                    environment_name: EnvironmentName("env".into()),
                    deployment_revision: DeploymentRevision::INITIAL,
                    domain: domain.clone(),
                    security_scheme_name: None,
                    security_scheme: None,
                    registered_agent_types: vec![],
                    tools: vec![],
                }))),
                "broken.example.com" => Err(McpCapabilitiesLookupError::InternalError(
                    anyhow::anyhow!("registry unavailable"),
                )),
                _ => Err(McpCapabilitiesLookupError::UnknownSite(domain.clone())),
            }
        }
        async fn invalidate(&self, _: &Domain) {}
        async fn invalidate_all(&self) {}
    }

    fn mcp_only() -> McpOnlyEndpoint {
        McpOnlyEndpoint::new(
            TrustedIngress::new(&["10.0.0.1/32".parse().unwrap()]),
            Arc::new(Lookup),
            make_sync(|req: Request| {
                let mcp_request = req.extensions().get::<McpRequest>().unwrap();
                format!(
                    "{} {}",
                    mcp_request.origin.authority, mcp_request.deployment.compiled.domain.0
                )
            })
            .map_to_response()
            .boxed(),
        )
    }

    async fn call(endpoint: &McpOnlyEndpoint, peer: &str, headers: &[(&str, &str)]) -> Response {
        let mut builder = http::Request::builder().uri("/mcp");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        endpoint
            .call(request_from_peer(peer, builder, Body::empty()))
            .await
            .unwrap()
    }

    #[test]
    async fn the_endpoint_serves_with_the_authenticated_deployment() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let (session_store, _db) =
            crate::custom_api::oidc::session_store::sqlite_test_session_store().await;
        // The lookup serves another deployment of the domain, with no tools; the requests must
        // be served with the deployment that the request was authenticated against.
        let endpoint = mcp_endpoint(
            Arc::new(Lookup),
            harness.worker_service.clone(),
            Arc::new(crate::custom_api::oidc::DefaultIdentityProvider),
            Arc::new(session_store),
            &crate::config::McpConfig::default(),
            usize::MAX,
        );
        let authenticated = McpRequest {
            origin: crate::ingress::PublicOrigin {
                scheme: "https".to_string(),
                authority: "tools.example.com".to_string(),
                forwarded: false,
            },
            deployment: Arc::new(McpDeployment::new(harness.compiled_mcp(
                "tools.example.com",
                DeploymentRevision::INITIAL,
                false,
            ))),
        };
        let post = |body: serde_json::Value| {
            let mut request = Request::builder()
                .method(http::Method::POST)
                .uri_str("/mcp")
                .header("host", "tools.example.com")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(serde_json::to_vec(&body).unwrap());
            request.extensions_mut().insert(authenticated.clone());
            request
        };

        let listed = endpoint
            .call(post(serde_json::json!(
                {"jsonrpc": "2.0", "id": 1, "method": "tools/list"}
            )))
            .await
            .unwrap();
        assert!(!listed.headers().contains_key("mcp-session-id"));
        let listed = streamed_messages(listed).await;
        assert_eq!(listed[0]["result"]["tools"][0]["name"], "mcp-agent-run");

        let called = endpoint
            .call(post(serde_json::json!(
                {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                 "params": {"name": "mcp-agent-run", "arguments": {}}}
            )))
            .await
            .unwrap();
        assert!(!called.headers().contains_key("mcp-session-id"));
        let called = streamed_messages(called).await;
        assert_eq!(called[0]["result"]["isError"], false, "{called:?}");
    }

    /// Serves the deployment that the requests are authenticated against, for every domain.
    struct OneDeployment(Arc<McpDeployment>);

    #[async_trait::async_trait]
    impl McpCapabilityLookup for OneDeployment {
        async fn get(&self, _: &Domain) -> Result<Arc<McpDeployment>, McpCapabilitiesLookupError> {
            Ok(self.0.clone())
        }
        async fn invalidate(&self, _: &Domain) {}
        async fn invalidate_all(&self) {}
    }

    const AGENT_TOOL: &str = "mcp-agent-run";
    const NATIVE_TOOL: &str = "native";
    const REVISION: (&str, &str) = ("MCP-Protocol-Version", "2025-06-18");
    const UNSERVED: (&str, &str) = ("MCP-Protocol-Version", "2024-11-05");
    const SERVED: [&str; 4] = ["2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28"];

    /// MCP endpoints over a harness, for the harness's deployment of `tools.example.com` with
    /// its agent tool and its native tool.
    struct Served {
        endpoint: BoxEndpoint<'static>,
        lookup: Arc<OneDeployment>,
        worker_service: Arc<WorkerService>,
        session_store: Arc<dyn SessionStore>,
        _db: tempfile::NamedTempFile,
    }

    impl Served {
        async fn new(harness: &crate::mcp::InvocationHarness) -> Self {
            let (session_store, db) =
                crate::custom_api::oidc::session_store::sqlite_test_session_store().await;
            let lookup = Arc::new(OneDeployment(Arc::new(McpDeployment::new(
                harness.compiled_mcp("tools.example.com", DeploymentRevision::INITIAL, true),
            ))));
            let worker_service = harness.worker_service.clone();
            let session_store: Arc<dyn SessionStore> = Arc::new(session_store);
            Self {
                endpoint: endpoint_over(&lookup, &worker_service, &session_store),
                lookup,
                worker_service,
                session_store,
                _db: db,
            }
        }

        /// Another MCP endpoint over the same lookup and worker service.
        fn another_endpoint(&self) -> BoxEndpoint<'static> {
            endpoint_over(&self.lookup, &self.worker_service, &self.session_store)
        }

        fn authenticated(&self) -> McpRequest {
            McpRequest {
                origin: PublicOrigin {
                    scheme: "https".to_string(),
                    authority: "tools.example.com".to_string(),
                    forwarded: false,
                },
                deployment: self.lookup.0.clone(),
            }
        }

        fn request(
            &self,
            method: http::Method,
            headers: &[(&str, &str)],
            body: Option<&Value>,
        ) -> Request {
            let builder = headers.iter().fold(
                Request::builder()
                    .method(method)
                    .uri_str("/mcp")
                    .header("host", "tools.example.com")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream"),
                |builder, (name, value)| builder.header(*name, *value),
            );
            let mut request = builder.body(
                body.map(|body| serde_json::to_vec(body).unwrap())
                    .unwrap_or_default(),
            );
            request.extensions_mut().insert(self.authenticated());
            request
        }

        async fn send(&self, request: Request) -> Response {
            self.endpoint.call(request).await.unwrap()
        }

        async fn post(&self, headers: &[(&str, &str)], body: Value) -> Response {
            self.send(self.request(http::Method::POST, headers, Some(&body)))
                .await
        }
    }

    fn endpoint_over(
        lookup: &Arc<OneDeployment>,
        worker_service: &Arc<WorkerService>,
        session_store: &Arc<dyn SessionStore>,
    ) -> BoxEndpoint<'static> {
        mcp_endpoint(
            lookup.clone(),
            worker_service.clone(),
            Arc::new(crate::custom_api::oidc::DefaultIdentityProvider),
            session_store.clone(),
            &crate::config::McpConfig::default(),
            usize::MAX,
        )
    }

    fn rpc(id: i64, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    fn initialize(revision: &str) -> Value {
        rpc(
            1,
            "initialize",
            json!({"protocolVersion": revision, "capabilities": {},
                   "clientInfo": {"name": "test", "version": "1"}}),
        )
    }

    fn call_params(name: &str) -> Value {
        json!({"name": name, "arguments": {}})
    }

    /// The `_meta` that a 2026-07-28 request carries in place of `initialize`.
    fn meta_2026() -> Value {
        json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28",
               "io.modelcontextprotocol/clientCapabilities": {}})
    }

    /// `params` with the 2026-07-28 `_meta`.
    fn with_meta_2026(params: Value) -> Value {
        let mut params = params;
        params["_meta"] = meta_2026();
        params
    }

    async fn json_body(response: Response) -> Value {
        serde_json::from_str(&response.into_body().into_string().await.unwrap()).unwrap()
    }

    fn tool_names(messages: &[Value]) -> std::collections::BTreeSet<String> {
        messages[0]["result"]["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool["name"].as_str().map(String::from))
            .collect()
    }

    #[test]
    async fn initialize_issues_no_session_id_and_names_a_served_revision() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let answer = served.post(&[], initialize("2024-11-05")).await;
        assert!(!answer.headers().contains_key("mcp-session-id"));
        let answer = streamed_messages(answer).await;
        assert_eq!(answer[0]["result"]["protocolVersion"], "2025-11-25");
    }

    #[test]
    async fn a_request_that_names_an_unserved_revision_gets_400_and_invokes_nothing() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let refused: Vec<(http::StatusCode, String)> = futures::stream::iter([
            rpc(1, "tools/list", json!({})),
            rpc(2, "tools/call", call_params(AGENT_TOOL)),
            rpc(3, "tools/call", call_params(NATIVE_TOOL)),
        ])
        .then(|body| async {
            let response = served.post(&[UNSERVED], body).await;
            (
                response.status(),
                response.into_body().into_string().await.unwrap(),
            )
        })
        .collect()
        .await;
        refused.iter().for_each(|(status, body)| {
            assert_eq!(*status, http::StatusCode::BAD_REQUEST, "{body}");
            assert!(body.contains("2024-11-05"), "{body}");
        });
        assert!(harness.contexts.lock().unwrap().is_empty());
        assert!(harness.native_session_keys().is_empty());
    }

    #[test]
    async fn each_served_revision_lists_the_same_tools() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let listed: Vec<std::collections::BTreeSet<String>> = futures::stream::iter(SERVED)
            .then(|revision| {
                let served = &served;
                async move {
                    let response = if revision == "2026-07-28" {
                        served
                            .post(
                                &[
                                    ("MCP-Protocol-Version", revision),
                                    ("Mcp-Method", "tools/list"),
                                ],
                                rpc(1, "tools/list", with_meta_2026(json!({}))),
                            )
                            .await
                    } else {
                        served
                            .post(
                                &[("MCP-Protocol-Version", revision)],
                                rpc(1, "tools/list", json!({})),
                            )
                            .await
                    };
                    tool_names(&streamed_messages(response).await)
                }
            })
            .collect()
            .await;
        let expected: std::collections::BTreeSet<String> =
            [AGENT_TOOL, NATIVE_TOOL].map(String::from).into();
        listed.iter().for_each(|names| assert_eq!(names, &expected));
    }

    #[test]
    async fn a_session_id_from_the_client_changes_nothing() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let session = ("Mcp-Session-Id", "0f9c2a4e-5b1d-4c8e-9a7f-3d2e1b0c9a8f");
        let answers: Vec<Vec<Value>> = futures::stream::iter([
            (vec![REVISION], rpc(1, "tools/list", json!({}))),
            (vec![REVISION, session], rpc(1, "tools/list", json!({}))),
            (
                vec![REVISION],
                rpc(2, "tools/call", call_params(AGENT_TOOL)),
            ),
            (
                vec![REVISION, session],
                rpc(2, "tools/call", call_params(AGENT_TOOL)),
            ),
        ])
        .then(|(headers, body)| {
            let served = &served;
            async move {
                let response = served.post(&headers, body).await;
                assert!(!response.headers().contains_key("mcp-session-id"));
                streamed_messages(response).await
            }
        })
        .collect()
        .await;
        assert_eq!(answers[0], answers[1]);
        assert_eq!(answers[2], answers[3]);
        assert_eq!(answers[2][0]["result"]["isError"], false, "{answers:?}");
    }

    #[test]
    async fn get_and_delete_get_405_with_allow_post() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let responses: Vec<Response> = futures::stream::iter([
            (http::Method::GET, vec![]),
            (http::Method::GET, vec![("Last-Event-ID", "0")]),
            (http::Method::GET, vec![UNSERVED]),
            (http::Method::DELETE, vec![("Mcp-Session-Id", "a-session")]),
            (http::Method::DELETE, vec![UNSERVED]),
        ])
        .then(|(method, headers)| served.send(served.request(method, &headers, None)))
        .collect()
        .await;
        responses.iter().for_each(|response| {
            assert_eq!(response.status(), http::StatusCode::METHOD_NOT_ALLOWED);
            assert_eq!(response.headers()["Allow"], "POST");
        });
    }

    #[test]
    async fn a_post_with_last_event_id_gets_its_answer_and_no_replay() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let answer = streamed_messages(
            served
                .post(
                    &[REVISION, ("Last-Event-ID", "0")],
                    rpc(4, "tools/list", json!({})),
                )
                .await,
        )
        .await;
        assert_eq!(answer.len(), 1, "{answer:?}");
        assert_eq!(answer[0]["id"], 4);
        assert!(tool_names(&answer).contains(AGENT_TOOL), "{answer:?}");
    }

    #[test]
    async fn server_discover_returns_the_served_revisions_and_the_capabilities() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let answer = streamed_messages(
            served
                .post(
                    &[
                        ("MCP-Protocol-Version", "2026-07-28"),
                        ("Mcp-Method", "server/discover"),
                    ],
                    rpc(1, "server/discover", json!({"_meta": meta_2026()})),
                )
                .await,
        )
        .await;
        let capabilities = serde_json::to_value(
            GolemAgentMcpServer::new(served.lookup.clone(), served.worker_service.clone())
                .get_info()
                .capabilities,
        )
        .unwrap();
        assert_eq!(
            answer[0]["result"]["supportedVersions"],
            json!(SERVED),
            "{answer:?}"
        );
        assert_eq!(answer[0]["result"]["capabilities"], capabilities);
    }

    #[test]
    async fn ping_is_answered_only_on_the_revisions_that_define_it() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let answers: Vec<Vec<Value>> = futures::stream::iter([
            vec![],
            vec![("MCP-Protocol-Version", "2025-03-26")],
            vec![REVISION],
            vec![("MCP-Protocol-Version", "2025-11-25")],
        ])
        .then(|headers| {
            let served = &served;
            async move {
                streamed_messages(served.post(&headers, rpc(5, "ping", json!({}))).await).await
            }
        })
        .collect()
        .await;
        answers
            .iter()
            .for_each(|answer| assert_eq!(answer[0]["result"], json!({}), "{answer:?}"));

        let refused = served
            .post(
                &[
                    ("MCP-Protocol-Version", "2026-07-28"),
                    ("Mcp-Method", "ping"),
                ],
                rpc(6, "ping", json!({"_meta": meta_2026()})),
            )
            .await;
        assert_eq!(refused.status(), http::StatusCode::NOT_FOUND);
        assert_eq!(json_body(refused).await["error"]["code"], -32601);
    }

    /// Starts a native call that answers when the test releases it, and waits until its tool
    /// session is open. Returns the call's response and the rest of the session's request
    /// stream, which ends when the call returns.
    async fn held_native_call(
        served: &Served,
        harness: &crate::mcp::InvocationHarness,
    ) -> (Response, crate::service::worker::InvocationRequestStream) {
        harness.answer_native_sessions(
            crate::mcp::invoke::test_support::NativeSessions::HoldUntilReleased,
        );
        let call = served
            .post(&[REVISION], rpc(7, "tools/call", call_params(NATIVE_TOOL)))
            .await;
        harness.native_sessions_opened(1).await;
        (call, harness.take_native_session_tail())
    }

    const RUNNING: std::time::Duration = std::time::Duration::from_millis(300);
    const WAIT: std::time::Duration = std::time::Duration::from_secs(5);

    #[test]
    #[test_r::timeout("30s")]
    async fn notifications_cancelled_cancels_nothing_and_the_call_completes() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let (call, mut tail) = held_native_call(&served, &harness).await;

        let cancelled = served
            .post(
                &[REVISION],
                json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                       "params": {"requestId": 7, "reason": "test"}}),
            )
            .await;
        assert_eq!(cancelled.status(), http::StatusCode::ACCEPTED);
        assert!(
            tokio::time::timeout(RUNNING, tail.next()).await.is_err(),
            "the call still runs"
        );

        harness.release_native_session();
        let answer = streamed_messages(call).await;
        assert_eq!(answer[0]["result"]["isError"], false, "{answer:?}");
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn a_closed_response_stream_cancels_nothing_and_the_call_completes() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let (call, mut tail) = held_native_call(&served, &harness).await;

        drop(call);
        assert!(
            tokio::time::timeout(RUNNING, tail.next()).await.is_err(),
            "a closed response stream cancels nothing"
        );

        harness.release_native_session();
        assert!(
            tokio::time::timeout(WAIT, tail.next())
                .await
                .expect("the call completes")
                .is_none()
        );
    }

    #[test]
    async fn two_endpoints_over_one_lookup_serve_initialize_on_one_and_a_call_on_the_other() {
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let mcp_only = |mcp: BoxEndpoint<'static>| {
            McpOnlyEndpoint::new(
                TrustedIngress::new(&["10.0.0.1/32".parse().unwrap()]),
                served.lookup.clone(),
                mcp,
            )
        };
        let (first, second) = (
            mcp_only(served.another_endpoint()),
            mcp_only(served.another_endpoint()),
        );
        let post = |headers: &[(&str, &str)], body: Value| {
            let builder = headers.iter().fold(
                http::Request::builder()
                    .method(http::Method::POST)
                    .uri("/mcp")
                    .header("Host", "tools.example.com")
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json, text/event-stream"),
                |builder, (name, value)| builder.header(*name, *value),
            );
            request_from_peer(
                "10.0.0.2:5000",
                builder,
                Body::from_vec(serde_json::to_vec(&body).unwrap()),
            )
        };

        let initialized = streamed_messages(
            first
                .call(post(&[], initialize("2025-06-18")))
                .await
                .unwrap(),
        )
        .await;
        let revision = initialized[0]["result"]["protocolVersion"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(revision, "2025-06-18");
        let called = streamed_messages(
            second
                .call(post(
                    &[("MCP-Protocol-Version", &revision)],
                    rpc(2, "tools/call", call_params(AGENT_TOOL)),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(called[0]["result"]["isError"], false, "{called:?}");
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn a_2026_client_and_a_2025_client_list_and_call_the_same_tools() {
        use rmcp::model::{
            CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation,
            ProtocolVersion,
        };
        use rmcp::transport::StreamableHttpClientTransport;
        use rmcp::{ClientLifecycleMode, ClientServiceExt, ServiceExt};

        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let served = Served::new(&harness).await;
        let authenticated = served.authenticated();
        let endpoint = served
            .another_endpoint()
            .before(move |mut request: Request| {
                request.extensions_mut().insert(authenticated.clone());
                async move { Ok(request) }
            })
            .map_to_response();
        let (address, _server) = crate::transport_test_support::server(endpoint).await;
        let url = format!("http://{address}/mcp");
        let client_info = || {
            ClientConfig::new(
                ClientCapabilities::default(),
                Implementation::new("era-test", "1"),
            )
        };

        let modern = client_info()
            .serve_with_lifecycle(
                StreamableHttpClientTransport::from_uri(url.clone()),
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await
            .unwrap();
        let legacy = client_info()
            .with_protocol_version(ProtocolVersion::V_2025_06_18)
            .serve(StreamableHttpClientTransport::from_uri(url))
            .await
            .unwrap();
        assert_eq!(
            modern.peer_info().unwrap().protocol_version,
            ProtocolVersion::V_2026_07_28
        );
        assert_eq!(
            legacy.peer_info().unwrap().protocol_version,
            ProtocolVersion::V_2025_06_18
        );

        let names = |tools: Vec<rmcp::model::Tool>| {
            tools
                .into_iter()
                .map(|tool| tool.name.into_owned())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let modern_tools = names(modern.list_all_tools().await.unwrap());
        let legacy_tools = names(legacy.list_all_tools().await.unwrap());
        assert_eq!(modern_tools, legacy_tools);
        assert!(modern_tools.contains(AGENT_TOOL), "{modern_tools:?}");

        let call = || CallToolRequestParams::new(AGENT_TOOL).with_arguments(Default::default());
        assert_eq!(
            modern.call_tool(call()).await.unwrap().is_error,
            Some(false)
        );
        assert_eq!(
            legacy.call_tool(call()).await.unwrap().is_error,
            Some(false)
        );
        modern.cancel().await.unwrap();
        legacy.cancel().await.unwrap();
    }

    #[test]
    async fn serves_mcp_deployments_with_their_resolved_origin() {
        let endpoint = mcp_only();
        let response = call(&endpoint, "10.0.0.2:5000", &[("Host", "Tools.Example.com")]).await;
        assert_eq!(response.status(), http::StatusCode::OK);
        assert_eq!(
            response.into_body().into_string().await.unwrap(),
            "tools.example.com tools.example.com"
        );
    }

    #[test]
    async fn rejects_invalid_origins_unknown_domains_and_failed_lookups() {
        let endpoint = mcp_only();
        assert_eq!(
            call(&endpoint, "10.0.0.2:5000", &[]).await.status(),
            http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            call(&endpoint, "10.0.0.2:5000", &[("Host", "other.example.com")])
                .await
                .status(),
            http::StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(
                &endpoint,
                "10.0.0.2:5000",
                &[("Host", "broken.example.com")]
            )
            .await
            .status(),
            http::StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    async fn forwarded_host_is_honored_only_from_a_trusted_ingress() {
        let endpoint = mcp_only();
        let headers = [
            ("Host", "worker-service:9007"),
            ("X-Forwarded-Host", "tools.example.com"),
            ("X-Forwarded-Proto", "https"),
        ];
        assert_eq!(
            call(&endpoint, "10.0.0.1:5000", &headers).await.status(),
            http::StatusCode::OK
        );
        assert_eq!(
            call(&endpoint, "10.0.0.2:5000", &headers).await.status(),
            http::StatusCode::NOT_FOUND
        );
    }
}
