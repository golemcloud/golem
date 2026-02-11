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

use std::collections::HashMap;
use std::iter::Map;
use std::slice::Iter;
use super::call_agent::CallAgentHandler;
use super::cors::{apply_cors_outgoing_middleware, handle_cors_preflight_behaviour};
use super::error::RequestHandlerError;
use super::model::RichRouteBehaviour;
use super::openapi_handler::OpenApiHandler;
use super::route_resolver::{ResolvedRouteEntry, RouteResolver};
use super::security::handler::OidcHandler;
use super::webhoooks::WebhookCallbackHandler;
use super::{OidcCallbackBehaviour, ResponseBody, RichCompiledRoute, RouteExecutionResult};
use crate::custom_api::RichRequest;
use anyhow::anyhow;
use golem_service_base::custom_api::CorsPreflightBehaviour;
use golem_service_base::custom_api::OpenApiSpecBehaviour;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use poem::{Request, Response};
use std::sync::Arc;
use http::StatusCode;
use tracing::{Instrument, debug};
use golem_common::base_model::agent::AgentType;

pub struct RequestHandler {
    route_resolver: Arc<RouteResolver>,
    call_agent_handler: Arc<CallAgentHandler>,
    oidc_handler: Arc<OidcHandler>,
    webhook_callback_handler: Arc<WebhookCallbackHandler>,
}

#[allow(irrefutable_let_patterns)]
impl RequestHandler {
    pub fn new(
        route_resolver: Arc<RouteResolver>,
        call_agent_handler: Arc<CallAgentHandler>,
        oidc_handler: Arc<OidcHandler>,
        webhook_callback_handler: Arc<WebhookCallbackHandler>,
    ) -> Self {
        Self {
            route_resolver,
            call_agent_handler,
            oidc_handler,
            webhook_callback_handler,
        }
    }

    pub async fn handle_request(&self, request: Request) -> Result<Response, RequestHandlerError> {
        debug!("Begin http request handling for request {request:?}");

        let matching_route = self.route_resolver.resolve_matching_route(&request).await?;
        let mut request = RichRequest::new(request);

        let execution_result = self
            .execute_route_and_middlewares(&mut request, &matching_route)
            .instrument(tracing::span!(
                tracing::Level::INFO,
                "handle_route",
                domain = %matching_route.domain,
                method = %matching_route.route.method,
                route = %matching_route.route.path.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("/")
            ))
            .await?;

        let response = route_execution_result_to_response(execution_result)?;

        Ok(response)
    }

    async fn execute_route_and_middlewares(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        if let Some(short_circuit) = self
            .oidc_handler
            .apply_oidc_incoming_middleware(request, resolved_route)
            .await?
        {
            return Ok(short_circuit);
        }

        let mut result = self.execute_route(request, resolved_route).await?;

        apply_cors_outgoing_middleware(&mut result, request, resolved_route).await?;

        Ok(result)
    }

    async fn execute_route(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        match &resolved_route.route.behavior {
            RichRouteBehaviour::CallAgent(behaviour) => {
                self.call_agent_handler
                    .handle_call_agent_behaviour(request, resolved_route, behaviour)
                    .await
            }

            RichRouteBehaviour::CorsPreflight(CorsPreflightBehaviour {
                allowed_origins,
                allowed_methods,
            }) => handle_cors_preflight_behaviour(request, allowed_origins, allowed_methods),

            RichRouteBehaviour::OidcCallback(OidcCallbackBehaviour { security_scheme }) => {
                self.oidc_handler
                    .handle_oidc_callback_behaviour(request, security_scheme)
                    .await
            }

            RichRouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour { open_api_spec }) => {
                match self
                    .route_resolver
                    .get_enriched_routes_for_domain(&resolved_route.domain)
                    .await
                {
                    Ok(routes) => {
                        let result: Vec<(AgentType, RichCompiledRoute)> =
                            routes
                                .iter()
                                .filter_map(|rich_compiled_route| {
                                    open_api_spec
                                        .iter()
                                        .flat_map(|r| r.routes.iter())
                                        .find(|route| route.route_id == rich_compiled_route.route_id)
                                        .map(|route| (route.agent_type.clone(), rich_compiled_route.clone()))
                                })
                                .collect();


                        match OpenApiHandler::generate_spec(result, &HashMap::new()).await {
                            Ok(yaml) => {
                                Ok(RouteExecutionResult {
                                    status: StatusCode::OK,
                                    headers: HashMap::new(),
                                    body: ResponseBody::PlainText {
                                        body: yaml
                                    },
                                })
                            }
                            Err(e) => Err(RequestHandlerError::OpenApiSpecGenerationFailed { error: e }),
                        }
                    }
                    Err(_) => Err(RequestHandlerError::OpenApiSpecGenerationFailed { error: "Failed to retrieve routes for domain".to_string() }),
                }
            }

            RichRouteBehaviour::WebhookCallback(behaviour) => {
                self.webhook_callback_handler
                    .handle_webhook_callback_behaviour(request, resolved_route, behaviour)
                    .await
            }
        }
    }
}

fn route_execution_result_to_response(
    result: RouteExecutionResult,
) -> Result<Response, RequestHandlerError> {
    let mut response_builder = Response::builder().status(result.status);

    for (name, value) in result.headers {
        response_builder = response_builder.header(name, value);
    }

    match result.body {
        ResponseBody::NoBody => Ok(response_builder.finish()),

        ResponseBody::ComponentModelJsonBody { body } => {
            let body = poem::Body::from_json(
                body.to_json_value()
                    .map_err(|e| anyhow!("ComponentModelJsonBody conversion error: {e}"))?,
            )
            .map_err(anyhow::Error::from)?;

            Ok(response_builder.body(body))
        }

        ResponseBody::UnstructuredBinaryBody { body } => Ok(response_builder
            .body(body.data)
            .set_content_type(body.binary_type.mime_type)),

        ResponseBody::PlainText(body) => Ok(response_builder.body(body)),
    }
}
