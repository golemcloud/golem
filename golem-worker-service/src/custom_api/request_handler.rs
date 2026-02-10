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

use super::call_agent::CallAgentHandler;
use super::cors::{apply_cors_outgoing_middleware, handle_cors_preflight_behaviour};
use super::error::RequestHandlerError;
use super::model::RichRouteBehaviour;
use super::openapi_handler::OpenApiHandler;
use super::route_resolver::{ResolvedRouteEntry, RouteResolver};
use super::security::handler::OidcHandler;
use super::{OidcCallbackBehaviour, ResponseBody, RouteExecutionResult};
use crate::custom_api::{ParsedRequestBody, RichRequest};
use crate::service::worker::WorkerService;
use anyhow::anyhow;
use golem_service_base::custom_api::{
    AgentWebhookId, CorsPreflightBehaviour, OpenApiSpecBehaviour, RequestBodySchema,
    WebhookCallbackBehaviour,
};
use golem_service_base::model::auth::AuthCtx;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use http::StatusCode;
use poem::{Request, Response};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{Instrument, debug};

pub struct RequestHandler {
    route_resolver: Arc<RouteResolver>,
    call_agent_handler: Arc<CallAgentHandler>,
    oidc_handler: Arc<OidcHandler>,
    worker_service: Arc<WorkerService>,
}

#[allow(irrefutable_let_patterns)]
impl RequestHandler {
    pub fn new(
        route_resolver: Arc<RouteResolver>,
        call_agent_handler: Arc<CallAgentHandler>,
        oidc_handler: Arc<OidcHandler>,
        worker_service: Arc<WorkerService>,
    ) -> Self {
        Self {
            route_resolver,
            call_agent_handler,
            oidc_handler,
            worker_service,
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

            RichRouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour { agent_type }) => {
                // Get all routes for this domain and generate OpenAPI spec
                match self
                    .route_resolver
                    .get_routes_for_domain(&resolved_route.domain)
                    .await
                {
                    Ok(routes) => {
                        match OpenApiHandler::generate_spec(agent_type, &routes) {
                            Ok(yaml) => {
                                let response = Response::builder()
                                    .content_type("application/yaml; charset=utf-8")
                                    .body(yaml);
                                Ok(RouteExecutionResult {
                                    status: StatusCode::OK,
                                    headers: HashMap::new(),
                                    body: ResponseBody::PlainText(
                                        response.finish().into_string().ok(),
                                    ),
                                })
                            }
                            Err(e) => Err(RequestHandlerError::InvalidRequest(format!(
                                "Failed to generate OpenAPI spec: {}",
                                e
                            ))),
                        }
                    }
                    Err(_) => Err(RequestHandlerError::InvalidRequest(
                        "Failed to fetch routes for OpenAPI spec generation".to_string(),
                    )),
                }
            }

            RichRouteBehaviour::WebhookCallback(WebhookCallbackBehaviour { component_id }) => {
                let webhook_id_segment = resolved_route.captured_path_parameters.first().ok_or(
                    RequestHandlerError::invariant_violated(
                        "no variable path segments for webhook callback ",
                    ),
                )?;
                let webhook_id =
                    AgentWebhookId::from_base64_url(webhook_id_segment).map_err(|_| {
                        RequestHandlerError::ValueParsingFailed {
                            value: webhook_id_segment.clone(),
                            expected: "AgentWebhookId",
                        }
                    })?;
                let promise_id = webhook_id.into_promise_id(*component_id);

                let body = request
                    .parse_request_body(&RequestBodySchema::UnrestrictedBinary)
                    .await?;
                let ParsedRequestBody::UnstructuredBinary(mut body_data) = body else {
                    return Err(RequestHandlerError::invariant_violated(
                        "UnrestrictedBinary body parsing yielded wrong type",
                    ));
                };
                let body_binary = body_data.take().map(|bs| bs.data).unwrap_or_default();

                let auth_ctx = AuthCtx::impersonated_user(resolved_route.route.account_id);

                tracing::debug!("Completing promise due to webhook_callback: {promise_id}");
                self.worker_service
                    .complete_promise(
                        &promise_id.worker_id,
                        promise_id.oplog_idx.as_u64(),
                        body_binary,
                        auth_ctx,
                    )
                    .await?;

                Ok(RouteExecutionResult {
                    status: StatusCode::NO_CONTENT,
                    headers: HashMap::new(),
                    body: ResponseBody::NoBody,
                })
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
    }
}
