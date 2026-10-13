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

use super::*;
use crate::config::RouteResolverConfig;
use crate::custom_api::api_definition_lookup::{
    ApiDefinitionLookupError, HttpApiDefinitionsLookup,
};
use crate::ingress::tests::request_from_peer;
use crate::mcp::{McpCapabilitiesLookupError, McpRequest, mcp_endpoint};
use golem_common::model::AgentInvocationOutput;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::application::ApplicationName;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::schema::{AgentConstructorSchema, InputSchema, SchemaValue};
use golem_service_base::custom_api::CompiledRoutes;
use golem_service_base::mcp::CompiledMcp;
use poem::endpoint::make_sync;
use poem::http::{Method, StatusCode, Version};
use poem::{Body, EndpointExt};
use serde_json::json;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use test_r::test;

const SHOP: &str = "shop.example.com";
const TOOLS: &str = "tools.example.com";
const BROKEN: &str = "broken.example.com";
/// An MCP domain whose HTTP API lookup fails.
const FLAKY_TOOLS: &str = "flaky-tools.example.com";
const INGRESS: &str = "10.0.0.1";

#[derive(Default)]
struct HttpApiDomains {
    lookups: Mutex<Vec<Domain>>,
}

#[async_trait::async_trait]
impl HttpApiDefinitionsLookup for HttpApiDomains {
    async fn get(&self, domain: &Domain) -> Result<CompiledRoutes, ApiDefinitionLookupError> {
        self.lookups.lock().unwrap().push(domain.clone());
        match domain.0.as_str() {
            FLAKY_TOOLS => Err(ApiDefinitionLookupError::InternalError(anyhow::anyhow!(
                "registry unavailable"
            ))),
            SHOP => Ok(CompiledRoutes {
                account_id: AccountId::new(),
                account_email: AccountEmail::new("owner@example.com"),
                environment_id: EnvironmentId::new(),
                deployment_revision: DeploymentRevision::INITIAL,
                security_schemes: HashMap::new(),
                routes: vec![],
            }),
            _ => Err(ApiDefinitionLookupError::UnknownSite(domain.clone())),
        }
    }
}

#[derive(Default)]
struct McpDomains {
    lookups: Mutex<Vec<Domain>>,
}

#[async_trait::async_trait]
impl McpCapabilityLookup for McpDomains {
    async fn get(&self, domain: &Domain) -> Result<CompiledMcp, McpCapabilitiesLookupError> {
        self.lookups.lock().unwrap().push(domain.clone());
        match domain.0.as_str() {
            TOOLS | FLAKY_TOOLS => Ok(compiled_mcp(domain)),
            BROKEN => Err(McpCapabilitiesLookupError::InternalError(anyhow::anyhow!(
                "registry unavailable"
            ))),
            _ => Err(McpCapabilitiesLookupError::UnknownSite(domain.clone())),
        }
    }
    async fn invalidate(&self, _: &Domain) {}
    async fn invalidate_all(&self) {}
}

fn compiled_mcp(domain: &Domain) -> CompiledMcp {
    CompiledMcp {
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
    }
}

/// Answers with the name of the endpoint that served the request and, for MCP requests,
/// the origin and deployment domain it received.
fn recording_endpoint(name: &'static str) -> BoxEndpoint<'static> {
    make_sync(move |req: Request| {
        let mcp = req.extensions().get::<McpRequest>().map(|mcp| {
            json!({
                "scheme": mcp.origin.scheme,
                "authority": mcp.origin.authority,
                "forwarded": mcp.origin.forwarded,
                "deployment": mcp.deployment.domain.0,
            })
        });
        poem::web::Json(json!({ "endpoint": name, "path": req.uri().path(), "mcp": mcp }))
    })
    .map_to_response()
    .boxed()
}

fn dispatcher(mcp: BoxEndpoint<'static>) -> (PublicEndpoint, Arc<McpDomains>) {
    let (endpoint, _, mcp_domains) = dispatcher_with(
        RouteResolverConfig::default(),
        |_| recording_endpoint("custom"),
        mcp,
    );
    (endpoint, mcp_domains)
}

fn dispatcher_with(
    route_resolver_config: RouteResolverConfig,
    custom_api: impl FnOnce(Arc<RouteResolver>) -> BoxEndpoint<'static>,
    mcp: BoxEndpoint<'static>,
) -> (PublicEndpoint, Arc<HttpApiDomains>, Arc<McpDomains>) {
    let http_api_domains = Arc::new(HttpApiDomains::default());
    let mcp_domains = Arc::new(McpDomains::default());
    let route_resolver = Arc::new(RouteResolver::new(
        &route_resolver_config,
        http_api_domains.clone(),
    ));
    let endpoint = PublicEndpoint::new(
        TrustedIngress::new(&[INGRESS.parse::<IpAddr>().unwrap()]),
        route_resolver.clone(),
        mcp_domains.clone(),
        custom_api(route_resolver),
        mcp,
    );
    (endpoint, http_api_domains, mcp_domains)
}

/// A custom request handler that resolves the request's domain like the real one does.
fn resolving_endpoint(route_resolver: Arc<RouteResolver>) -> BoxEndpoint<'static> {
    poem::endpoint::make(move |req: Request| {
        let route_resolver = route_resolver.clone();
        async move {
            let outcome = match route_resolver.resolve_matching_route(&req).await {
                Ok(_) => "route".to_string(),
                Err(error) => format!("{error:?}"),
            };
            poem::web::Json(json!({ "endpoint": "custom", "outcome": outcome }))
        }
    })
    .map_to_response()
    .boxed()
}

fn request(peer: &str, host: &str, path: &str) -> PeerRequest {
    PeerRequest {
        peer: format!("{peer}:5000"),
        builder: poem::http::Request::builder()
            .uri(path)
            .header("Host", host),
        body: Body::empty(),
    }
}

struct PeerRequest {
    peer: String,
    builder: poem::http::request::Builder,
    body: Body,
}

impl PeerRequest {
    fn header(mut self, name: &str, value: &str) -> Self {
        self.builder = self.builder.header(name, value);
        self
    }

    fn method(mut self, method: Method) -> Self {
        self.builder = self.builder.method(method);
        self
    }

    fn body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    fn finish(self) -> Request {
        request_from_peer(&self.peer, self.builder, self.body)
    }
}

async fn served(endpoint: &PublicEndpoint, req: Request) -> serde_json::Value {
    let response = endpoint.call(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&response.into_body().into_bytes().await.unwrap()).unwrap()
}

#[test]
async fn http_api_domain_serves_every_path_with_the_custom_handler() {
    let (endpoint, mcp_domains) = dispatcher(recording_endpoint("mcp"));
    for path in [
        "/orders/42",
        "/mcp",
        "/mcp/oauth/token",
        "/.well-known/oauth-protected-resource",
        "/v1/components",
    ] {
        let body = served(&endpoint, request("10.0.0.2", SHOP, path).finish()).await;
        assert_eq!(body["endpoint"], "custom", "{path}");
        assert_eq!(body["path"], path);
    }
    assert!(mcp_domains.lookups.lock().unwrap().is_empty());
}

#[test]
async fn mcp_domain_goes_to_the_mcp_endpoint_with_its_deployment_and_origin() {
    let (endpoint, _) = dispatcher(recording_endpoint("mcp"));
    let body = served(
        &endpoint,
        request("10.0.0.2", "Tools.Example.com", "/mcp").finish(),
    )
    .await;
    assert_eq!(
        body,
        json!({
            "endpoint": "mcp",
            "path": "/mcp",
            "mcp": {
                "scheme": "http",
                "authority": TOOLS,
                "forwarded": false,
                "deployment": TOOLS,
            },
        })
    );
}

#[test]
async fn unknown_domain_and_invalid_origin_go_to_the_custom_handler() {
    let (endpoint, _) = dispatcher(recording_endpoint("mcp"));
    let body = served(
        &endpoint,
        request("10.0.0.2", "other.example.com", "/mcp").finish(),
    )
    .await;
    assert_eq!(body["endpoint"], "custom");

    let body = served(
        &endpoint,
        Request::builder().uri("/mcp".parse().unwrap()).finish(),
    )
    .await;
    assert_eq!(body["endpoint"], "custom");
}

#[test]
async fn failed_mcp_lookup_is_unavailable() {
    let (endpoint, _) = dispatcher(recording_endpoint("mcp"));
    let response = endpoint
        .call(request("10.0.0.2", BROKEN, "/mcp").finish())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
async fn untrusted_peer_cannot_route_with_forwarded_headers() {
    let (endpoint, mcp_domains) = dispatcher(recording_endpoint("mcp"));
    let body = served(
        &endpoint,
        request("10.0.0.2", SHOP, "/mcp")
            .header("X-Forwarded-Host", TOOLS)
            .header("X-Forwarded-Proto", "https")
            .finish(),
    )
    .await;
    assert_eq!(body["endpoint"], "custom");
    assert!(mcp_domains.lookups.lock().unwrap().is_empty());
}

#[test]
async fn trusted_ingress_routes_with_forwarded_headers() {
    let (endpoint, _) = dispatcher(recording_endpoint("mcp"));
    let body = served(
        &endpoint,
        request(INGRESS, "worker-service:9006", "/mcp")
            .header("X-Forwarded-Host", TOOLS)
            .header("X-Forwarded-Proto", "https")
            .finish(),
    )
    .await;
    assert_eq!(body["endpoint"], "mcp");
    assert_eq!(body["mcp"]["scheme"], "https");
    assert_eq!(body["mcp"]["forwarded"], true);
}

#[test]
async fn http2_request_routes_by_its_authority() {
    let (endpoint, _) = dispatcher(recording_endpoint("mcp"));
    let body = served(
        &endpoint,
        Request::builder()
            .version(Version::HTTP_2)
            .uri(format!("http://{TOOLS}/mcp").parse().unwrap())
            .finish(),
    )
    .await;
    assert_eq!(body["endpoint"], "mcp");
}

async fn real_mcp_endpoint() -> (BoxEndpoint<'static>, tempfile::NamedTempFile) {
    let db = tempfile::NamedTempFile::new().unwrap();
    let harness = crate::mcp::InvocationHarness::new(
        AgentInvocationOutput {
            result: golem_common::model::AgentInvocationResult::AgentMethod {
                output: SchemaValue::Tuple { elements: vec![] },
            },
            consumed_fuel: None,
            invocation_status: None,
            component_revision: None,
            agent_id: None,
            idempotency_key: None,
            oplog_index: None,
            agent_fingerprint: None,
        },
        AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        vec![],
    );
    let endpoint = mcp_endpoint(
        Arc::new(McpDomains::default()),
        harness.worker_service.clone(),
        Arc::new(crate::custom_api::oidc::DefaultIdentityProvider),
        Arc::new(
            crate::custom_api::oidc::session_store::SqliteSessionStore::configured(
                &crate::config::SqliteSessionStoreConfig {
                    pending_login_expiration: std::time::Duration::from_secs(60),
                    cleanup_interval: std::time::Duration::from_secs(60),
                    sqlite_config: golem_common::config::DbSqliteConfig {
                        database: db.path().to_string_lossy().into_owned(),
                        max_connections: 1,
                        foreign_keys: false,
                    },
                },
            )
            .await
            .unwrap(),
        ),
    );
    (endpoint, db)
}

#[test]
#[test_r::timeout("30s")]
async fn mcp_domain_serves_only_the_mcp_routes() {
    // Unmatched MCP paths are poem errors, which the listener turns into 404 responses.
    let (mcp, _db) = real_mcp_endpoint().await;
    let (endpoint, _) = dispatcher(mcp);

    let response = endpoint
        .call(
            request("10.0.0.2", TOOLS, "/mcp")
                .method(Method::POST)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .body(
                    Body::from_json(json!({
                        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                            "protocolVersion": "2025-03-26", "capabilities": {},
                            "clientInfo": {"name": "test", "version": "1"}
                        }
                    }))
                    .unwrap(),
                )
                .finish(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key("Mcp-Session-Id"));

    let response = endpoint
        .get_response(request("10.0.0.2", TOOLS, "/orders/42").finish())
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // The deployment has no security scheme, so there is no OAuth metadata to serve.
    let response = endpoint
        .get_response(request("10.0.0.2", TOOLS, "/.well-known/oauth-protected-resource").finish())
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
async fn the_custom_handler_reuses_the_admitted_routing_snapshot() {
    let uncached = RouteResolverConfig {
        router_cache_max_capacity: 0,
        ..RouteResolverConfig::default()
    };
    let (endpoint, http_api_domains, _) =
        dispatcher_with(uncached, resolving_endpoint, recording_endpoint("mcp"));

    let body = served(&endpoint, request("10.0.0.2", SHOP, "/orders/42").finish()).await;
    assert_eq!(body["endpoint"], "custom");
    assert_eq!(body["outcome"], "NoMatchingRoute");

    let body = served(
        &endpoint,
        request("10.0.0.2", "other.example.com", "/").finish(),
    )
    .await;
    assert_eq!(body["outcome"], "UnknownSite");

    assert_eq!(
        *http_api_domains.lookups.lock().unwrap(),
        vec![
            Domain(SHOP.to_string()),
            Domain("other.example.com".to_string())
        ]
    );
}

#[test]
async fn mcp_domain_is_served_when_its_http_api_lookup_fails() {
    let (endpoint, _) = dispatcher(recording_endpoint("mcp"));
    let body = served(&endpoint, request("10.0.0.2", FLAKY_TOOLS, "/mcp").finish()).await;
    assert_eq!(body["endpoint"], "mcp");
    assert_eq!(body["mcp"]["deployment"], FLAKY_TOOLS);
}
