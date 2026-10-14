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

use crate::custom_api::route_resolver::RouteResolver;
use crate::ingress::TrustedIngress;
use crate::mcp::{McpCapabilityLookup, McpDomainLookup, lookup_mcp_domain};
use golem_common::model::domain_registration::Domain;
use poem::endpoint::BoxEndpoint;
use poem::{Endpoint, IntoResponse, Request, Response, Result, http};
use std::sync::Arc;

/// Serves the deployments of every domain on one listener, chosen by the request's host.
///
/// A domain holds an HTTP API deployment or an MCP deployment, never both:
/// - a domain with an HTTP API deployment goes to the custom request handler, for every path;
/// - a domain with an MCP deployment goes to the MCP endpoint;
/// - a failed MCP lookup is answered with 503;
/// - every other request goes to the custom request handler, which rejects unknown domains and
///   invalid origins with its usual errors.
pub struct PublicEndpoint {
    trusted_ingress: TrustedIngress,
    route_resolver: Arc<RouteResolver>,
    mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
    custom_api: BoxEndpoint<'static>,
    mcp: BoxEndpoint<'static>,
}

impl PublicEndpoint {
    pub fn new(
        trusted_ingress: TrustedIngress,
        route_resolver: Arc<RouteResolver>,
        mcp_capability_lookup: Arc<dyn McpCapabilityLookup>,
        custom_api: BoxEndpoint<'static>,
        mcp: BoxEndpoint<'static>,
    ) -> Self {
        Self {
            trusted_ingress,
            route_resolver,
            mcp_capability_lookup,
            custom_api,
            mcp,
        }
    }
}

impl Endpoint for PublicEndpoint {
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        let Ok(origin) = self.trusted_ingress.public_origin(&req) else {
            return self.custom_api.call(req).await;
        };
        // The HTTP API lookup comes first. It admits the routing snapshot for the request, and
        // the custom request handler resolves its route from that same snapshot.
        let domain = Domain(origin.authority.clone());
        if let Ok(true) = self.route_resolver.admit_domain(&mut req, &domain).await {
            return self.custom_api.call(req).await;
        }
        // A failed HTTP API lookup still consults the MCP lookup, so an MCP request is not sent
        // to the custom request handler, which would answer 404 and end the MCP session.
        match lookup_mcp_domain(self.mcp_capability_lookup.as_ref(), origin).await {
            McpDomainLookup::Found(mcp_request) => {
                req.extensions_mut().insert(mcp_request);
                self.mcp.call(req).await.map(IntoResponse::into_response)
            }
            McpDomainLookup::NotFound => self.custom_api.call(req).await,
            McpDomainLookup::Failed => Ok(Response::builder()
                .status(http::StatusCode::SERVICE_UNAVAILABLE)
                .finish()),
        }
    }
}

#[cfg(test)]
mod tests;
