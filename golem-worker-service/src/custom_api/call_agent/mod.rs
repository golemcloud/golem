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

mod cache_headers;
mod parameter_parsing;
mod response_mapping;

use self::cache_headers::{
    add_vary_header, build_cache_control_value, build_etag_value, headers as cache_header,
    if_none_match_hits, parse_if_none_match_entries, supports_http_revalidation,
};
use self::parameter_parsing::{
    parse_path_segment_value, parse_path_segment_value_to_component_model,
    parse_query_or_header_value,
};
use self::response_mapping::interpret_agent_response;
use super::RichRequest;
use super::error::RequestHandlerError;
use super::model::{ResponseBody, RichRouteSecurity};
use super::route_resolver::ResolvedRouteEntry;
use super::{ParsedRequestBody, RouteExecutionResult};
use crate::service::worker::WorkerService;
use anyhow::anyhow;
use golem_common::model::OplogIndex;
use golem_common::model::agent::{
    BinaryReference, BinaryReferenceValue, ComponentModelElementValue, DataValue, ElementValue,
    ElementValues, OidcPrincipal, ParsedAgentId, Principal, ReadOnlyConfig, TextReference,
    TextReferenceValue, UntypedDataValue, UntypedElementValue,
};
use golem_common::model::{AgentId, IdempotencyKey};
use golem_service_base::custom_api::{CallAgentBehaviour, ConstructorParameter, MethodParameter};
use golem_service_base::model::auth::AuthCtx;
use golem_wasm::ValueAndType;
use http::{Method, StatusCode};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};
use uuid::Uuid;

pub struct CallAgentHandler {
    worker_service: Arc<WorkerService>,
}

impl CallAgentHandler {
    pub fn new(worker_service: Arc<WorkerService>) -> Self {
        Self { worker_service }
    }

    pub async fn handle_call_agent_behaviour(
        &self,
        request: &mut RichRequest,
        resolved_route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let agent_id = self.build_agent_id(resolved_route, behaviour)?;

        let request_method = request.underlying.method().clone();

        // For read-only methods bound to GET/HEAD routes with a revalidatable
        // cache policy, try to serve a `304 Not Modified` without invoking the
        // executor when the client's `If-None-Match` value matches the agent's
        // current oplog index. `get_metadata` is safe here because the
        // executor implementation reads cached metadata / persisted oplog
        // state and does **not** wake or load the worker.
        if let Some(read_only) = behaviour.read_only.as_ref()
            && is_cacheable_method(&request_method)
            && supports_http_revalidation(read_only)
            && let Some(not_modified) = self
                .try_handle_if_none_match(request, behaviour, &agent_id, read_only, resolved_route)
                .await?
        {
            return Ok(not_modified);
        }

        let parsed_body = request
            .parse_request_body(&resolved_route.route.body)
            .await?;

        let method_params =
            self.resolve_method_arguments(resolved_route, request, behaviour, parsed_body)?;

        debug!("Invoking agent {agent_id}");

        let method_params_data_value = UntypedDataValue::Tuple(method_params);

        let proto_method_parameters: golem_api_grpc::proto::golem::component::UntypedDataValue =
            method_params_data_value.into();

        let invocation_context = Some(golem_api_grpc::proto::golem::worker::InvocationContext {
            parent: None,
            env: Default::default(),
            tracing: Some(request.invocation_context().into()),
        });

        let principal = principal_from_request(request)?;
        debug!("Using principal for invocation: {principal:?}");
        let proto_principal: golem_api_grpc::proto::golem::component::Principal = principal.into();

        let agent_response = self
            .worker_service
            .invoke_agent(
                &agent_id,
                Some(behaviour.method_name.clone()),
                Some(proto_method_parameters),
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                None,
                Some(IdempotencyKey::fresh()),
                invocation_context,
                AuthCtx::agent(resolved_route.route.account_id),
                proto_principal,
                Some(resolved_route.route.environment_id),
            )
            .await?;

        debug!("Received agent response: {agent_response:?}");

        // The executor returns the oplog index that the response's body was
        // committed at. Using this index as the validator avoids TOCTOU:
        // because the cached entry stores the same index, a subsequent cache
        // hit produces the same ETag, and any non-read-only write between two
        // reads bumps the cache epoch and forces a new invocation whose
        // recorded index will be strictly greater.
        let read_only_oplog_index = agent_response.read_only_oplog_index;

        let agent_result = match agent_response.result {
            golem_common::model::AgentInvocationResult::AgentMethod { output } => Some(output),
            _ => None,
        };

        let mut route_result =
            interpret_agent_response(agent_result, &behaviour.expected_agent_response)?;

        // For read-only methods bound to a GET/HEAD route, add cache headers to
        // the successful response and (for HEAD) strip the body.
        if let Some(read_only) = behaviour.read_only.as_ref()
            && is_cacheable_method(&request_method)
            && route_result.status.is_success()
        {
            add_read_only_cache_headers(
                &mut route_result.headers,
                &agent_id,
                read_only_oplog_index,
                read_only,
                behaviour,
                resolved_route,
            );

            if request_method == Method::HEAD {
                route_result.body = ResponseBody::NoBody;
            }
        }

        debug!("Returning call agent route result: {route_result:?}");

        Ok(route_result)
    }

    /// Pre-invocation `If-None-Match` revalidation. When the request supplies
    /// an `If-None-Match` header containing the ETag for the agent's current
    /// oplog index, we short-circuit with `304 Not Modified` and do not touch
    /// the executor at all.
    async fn try_handle_if_none_match(
        &self,
        request: &RichRequest,
        behaviour: &CallAgentBehaviour,
        agent_id: &AgentId,
        read_only: &ReadOnlyConfig,
        resolved_route: &ResolvedRouteEntry,
    ) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
        let Some(raw) = request
            .headers()
            .get(&cache_header::IF_NONE_MATCH)
            .and_then(|h| h.to_str().ok())
        else {
            return Ok(None);
        };

        let entries = parse_if_none_match_entries(raw);
        if entries.is_empty() {
            return Ok(None);
        }

        let Some(current_oplog) = self
            .fetch_current_oplog_index(resolved_route, agent_id)
            .await?
        else {
            return Ok(None);
        };

        if !if_none_match_hits(&entries, agent_id, current_oplog) {
            return Ok(None);
        }

        let mut headers: HashMap<http::HeaderName, String> = HashMap::new();
        add_read_only_cache_headers(
            &mut headers,
            agent_id,
            Some(current_oplog),
            read_only,
            behaviour,
            resolved_route,
        );

        Ok(Some(RouteExecutionResult {
            status: StatusCode::NOT_MODIFIED,
            headers,
            body: ResponseBody::NoBody,
        }))
    }

    async fn fetch_current_oplog_index(
        &self,
        resolved_route: &ResolvedRouteEntry,
        agent_id: &AgentId,
    ) -> Result<Option<OplogIndex>, RequestHandlerError> {
        match self
            .worker_service
            .get_metadata(agent_id, AuthCtx::agent(resolved_route.route.account_id))
            .await
        {
            Ok(metadata) => Ok(Some(metadata.last_oplog_index)),
            Err(err) => {
                // Failing to fetch metadata must not block the response. We
                // simply skip the cache-headers / 304 path and let the regular
                // invocation flow execute.
                warn!(
                    "Skipping HTTP cache headers for agent {agent_id}: failed to fetch metadata: {err}"
                );
                Ok(None)
            }
        }
    }

    fn build_agent_id(
        &self,
        resolved_route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
    ) -> Result<AgentId, RequestHandlerError> {
        let CallAgentBehaviour {
            component_id,
            agent_type,
            constructor_parameters,
            phantom,
            ..
        } = behaviour;

        let mut values = Vec::with_capacity(constructor_parameters.len());

        for param in constructor_parameters {
            match param {
                ConstructorParameter::Path {
                    path_segment_index,
                    parameter_type,
                } => {
                    let raw = resolved_route.captured_path_parameters
                        [usize::from(*path_segment_index)]
                    .clone();

                    let value = parse_path_segment_value_to_component_model(raw, parameter_type)?;

                    values.push(ElementValue::ComponentModel(ComponentModelElementValue {
                        value: ValueAndType::new(value, parameter_type.into()),
                    }));
                }
            }
        }

        let data_value = DataValue::Tuple(ElementValues { elements: values });

        let phantom_id = phantom.then(Uuid::new_v4);

        let agent_id = ParsedAgentId::new(agent_type.clone(), data_value, phantom_id)
            .map_err(|e| RequestHandlerError::AgentResponseTypeMismatch { error: e })?;

        Ok(AgentId {
            component_id: *component_id,
            agent_id: agent_id.to_string(),
        })
    }

    fn resolve_method_arguments(
        &self,
        resolved_route: &ResolvedRouteEntry,
        request: &RichRequest,
        behaviour: &CallAgentBehaviour,
        mut body: ParsedRequestBody,
    ) -> Result<Vec<UntypedElementValue>, RequestHandlerError> {
        let query_params = request.query_params();
        let headers = request.headers();

        let mut values = Vec::with_capacity(behaviour.method_parameters.len());

        for param in &behaviour.method_parameters {
            let value = match param {
                MethodParameter::Path {
                    path_segment_index,
                    parameter_type,
                } => {
                    let raw = resolved_route.captured_path_parameters
                        [usize::from(*path_segment_index)]
                    .clone();

                    parse_path_segment_value(raw, parameter_type)?
                }

                MethodParameter::Query {
                    query_parameter_name,
                    parameter_type,
                } => {
                    let empty = Vec::new();
                    let vals = query_params.get(query_parameter_name).unwrap_or(&empty);

                    parse_query_or_header_value(vals, parameter_type)?
                }

                MethodParameter::Header {
                    header_name,
                    parameter_type,
                } => {
                    let vals = headers
                        .get_all(header_name)
                        .iter()
                        .map(|h| {
                            h.to_str().map(String::from).map_err(|_| {
                                RequestHandlerError::HeaderIsNotAscii {
                                    header_name: header_name.clone(),
                                }
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;

                    parse_query_or_header_value(&vals, parameter_type)?
                }

                MethodParameter::JsonObjectBodyField { field_index } => match &body {
                    ParsedRequestBody::JsonBody(golem_wasm::Value::Record(fields)) => {
                        UntypedElementValue::ComponentModel(
                            fields[usize::from(*field_index)].clone(),
                        )
                    }

                    ParsedRequestBody::JsonBody(_) => {
                        return Err(RequestHandlerError::invariant_violated(
                            "Inconsistent API definition: JSON field parameter but body is not an object",
                        ));
                    }

                    _ => {
                        return Err(RequestHandlerError::invariant_violated(
                            "JSON body parameter used but no JSON body schema",
                        ));
                    }
                },

                MethodParameter::UnstructuredBinaryBody => match &mut body {
                    ParsedRequestBody::UnstructuredBinary(binary_source) => {
                        let binary_source = binary_source.take().ok_or_else(|| {
                            RequestHandlerError::invariant_violated(
                                "Parsed body was already consumed",
                            )
                        })?;

                        UntypedElementValue::UnstructuredBinary(BinaryReferenceValue {
                            value: BinaryReference::Inline(binary_source),
                        })
                    }

                    _ => {
                        return Err(RequestHandlerError::invariant_violated(
                            "Binary body parameter used but no binary body schema",
                        ));
                    }
                },

                MethodParameter::UnstructuredTextBody => match &mut body {
                    ParsedRequestBody::UnstructuredText(text_source) => {
                        let text_source = text_source.take().ok_or_else(|| {
                            RequestHandlerError::invariant_violated(
                                "Parsed body was already consumed",
                            )
                        })?;

                        UntypedElementValue::UnstructuredText(TextReferenceValue {
                            value: TextReference::Inline(text_source),
                        })
                    }

                    _ => {
                        return Err(RequestHandlerError::invariant_violated(
                            "Text body parameter used but no text body schema",
                        ));
                    }
                },
            };

            values.push(value);
        }

        Ok(values)
    }
}

fn is_cacheable_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
}

/// Insert the cache headers for a read-only `AgentMethod` response into the
/// given header map: `Cache-Control`, optionally `ETag` (when the cache policy
/// supports HTTP revalidation), and `Vary` for every header that influences
/// the response (`Authorization` / session-header for principal-aware
/// methods, plus any header-bound method parameters).
fn add_read_only_cache_headers(
    headers: &mut HashMap<http::HeaderName, String>,
    agent_id: &AgentId,
    oplog_index: Option<OplogIndex>,
    read_only: &ReadOnlyConfig,
    behaviour: &CallAgentBehaviour,
    resolved_route: &ResolvedRouteEntry,
) {
    headers.insert(
        cache_header::CACHE_CONTROL,
        build_cache_control_value(read_only),
    );

    // `CachePolicy::NoCache` explicitly opts out of HTTP caching: no ETag, and
    // therefore no 304 path. Otherwise emit an ETag for the supplied oplog
    // index: on the `200` path this is the executor-reported post-commit (or
    // cache-entry) index of the read-only invocation; on the `304` path this
    // is the worker's `last_oplog_index` observed before short-circuiting.
    // Skip the ETag entirely if no index is available (e.g. metadata fetch
    // failed, or the executor did not return one).
    if supports_http_revalidation(read_only)
        && let Some(idx) = oplog_index
    {
        headers.insert(cache_header::ETAG, build_etag_value(agent_id, idx));
    }

    // Vary on every request header that may influence the response: the
    // principal-bound header for principal-aware methods, and any header-bound
    // method parameters bound by the route.
    if read_only.uses_principal {
        add_vary_header(
            headers,
            principal_vary_header_name(&resolved_route.route.security),
        );
    }
    for param in &behaviour.method_parameters {
        if let MethodParameter::Header { header_name, .. } = param {
            add_vary_header(headers, header_name);
        }
    }
}

/// Return the request header used to derive the caller's principal for the
/// route's security configuration. Defaults to `Authorization` for OIDC /
/// session-based security; uses the configured header name for
/// `SessionFromHeader` security.
fn principal_vary_header_name(security: &RichRouteSecurity) -> &str {
    match security {
        RichRouteSecurity::SessionFromHeader(s) => s.header_name.as_str(),
        RichRouteSecurity::None | RichRouteSecurity::SecurityScheme(_) => "Authorization",
    }
}

fn principal_from_request(request: &RichRequest) -> Result<Principal, RequestHandlerError> {
    match request.authenticated_session() {
        Some(session) => Ok(Principal::Oidc(OidcPrincipal {
            sub: session.subject.clone(),
            issuer: session.issuer.clone(),
            email: session.email.clone(),
            name: session.name.clone(),
            email_verified: session.email_verified,
            given_name: session.given_name.clone(),
            family_name: session.family_name.clone(),
            picture: session.picture.clone(),
            preferred_username: session.preferred_username.clone(),
            claims: serde_json::to_string(&session.claims)
                .map_err(|e| anyhow!("CoreIdTokenClaims serialization error: {e}"))?,
        })),
        None => Ok(Principal::anonymous()),
    }
}
