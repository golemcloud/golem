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
use super::{
    GolemAgentMcpServer, McpBearerAuth, McpCapabilitiesLookupError, McpCapabilityLookup,
    oauth_proxy_routes,
};
use crate::bootstrap::Services;
use crate::custom_api::oidc::IdentityProvider;
use crate::custom_api::oidc::session_store::SessionStore;
use crate::ingress::{PublicOrigin, TrustedIngress};
use crate::service::worker::WorkerService;
use golem_common::model::domain_registration::Domain;
use golem_service_base::mcp::CompiledMcp;
use poem::endpoint::{BoxEndpoint, TowerCompatExt};
use poem::middleware::Cors;
use poem::{Endpoint, EndpointExt, IntoResponse, Request, Response, Result, http};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::Arc;

/// The MCP deployment that serves a request, and the public origin the request arrived at.
///
/// The listener resolves both before the request reaches the MCP endpoint, so the MCP
/// handlers never read the host or the forwarded headers themselves.
#[derive(Clone)]
pub struct McpRequest {
    pub origin: PublicOrigin,
    pub deployment: Arc<CompiledMcp>,
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
        Ok(deployment) => McpDomainLookup::Found(McpRequest {
            origin,
            deployment: Arc::new(deployment),
        }),
        Err(McpCapabilitiesLookupError::UnknownSite(_)) => McpDomainLookup::NotFound,
        Err(McpCapabilitiesLookupError::InternalError(error)) => {
            tracing::warn!(error = %error, "Failed to look up the MCP deployment of a domain");
            McpDomainLookup::Failed
        }
    }
}

/// Builds the MCP endpoint: the OAuth proxy routes, the `/mcp` transport, and their CORS policy.
/// Every request must carry an [`McpRequest`] extension.
pub fn make_mcp_endpoint(services: &Services) -> BoxEndpoint<'static> {
    mcp_endpoint(
        services.mcp_capability_lookup.clone(),
        services.worker_service.clone(),
        services.identity_provider.clone(),
        services.session_store.clone(),
        &services.mcp_config,
    )
}

pub(crate) fn mcp_endpoint(
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
    worker_service: Arc<WorkerService>,
    identity_provider: Arc<dyn IdentityProvider>,
    session_store: Arc<dyn SessionStore>,
    config: &crate::config::McpConfig,
) -> BoxEndpoint<'static> {
    let service = StreamableHttpService::new(
        move || {
            Ok(GolemAgentMcpServer::new(
                mcp_capability_lookup.clone(),
                worker_service.clone(),
            ))
        },
        LocalSessionManager::default().into(),
        mcp_http_transport_config(config),
    );

    let oauth_routes = oauth_proxy_routes(identity_provider.clone(), session_store);

    let mcp_with_auth = service.compat().with(McpBearerAuth::new(identity_provider));

    // OAuth routes use .at() so they take priority over .nest("/mcp", ...)
    oauth_routes
        .nest("/mcp", mcp_with_auth)
        .with(
            Cors::new()
                .allow_methods(vec!["GET", "POST", "DELETE", "OPTIONS"])
                .allow_headers(vec![
                    "Content-Type",
                    "Authorization",
                    "Mcp-Session-Id",
                    "Accept",
                    "Last-Event-ID",
                ])
                .expose_headers(vec!["Mcp-Session-Id"]),
        )
        .map_to_response()
        .boxed()
}

pub(crate) fn mcp_http_transport_config(
    config: &crate::config::McpConfig,
) -> StreamableHttpServerConfig {
    // Export hosts are deployment domains, not a static loopback allowlist.
    // Export request limits are independent of the bounded outbound import client.
    StreamableHttpServerConfig::default()
        .with_sse_keep_alive(Some(config.sse_keep_alive_interval))
        .disable_allowed_hosts()
        .with_max_request_body_bytes(usize::MAX)
}

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
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::application::ApplicationName;
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use poem::Body;
    use poem::endpoint::make_sync;
    use test_r::test;

    struct Lookup;

    #[async_trait::async_trait]
    impl McpCapabilityLookup for Lookup {
        async fn get(&self, domain: &Domain) -> Result<CompiledMcp, McpCapabilitiesLookupError> {
            match domain.0.as_str() {
                "tools.example.com" => Ok(CompiledMcp {
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
                }),
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
            TrustedIngress::new(&["10.0.0.1".parse().unwrap()]),
            Arc::new(Lookup),
            make_sync(|req: Request| {
                let mcp_request = req.extensions().get::<McpRequest>().unwrap();
                format!(
                    "{} {}",
                    mcp_request.origin.authority, mcp_request.deployment.domain.0
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
