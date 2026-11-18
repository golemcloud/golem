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
    CacheVisibility, add_vary_header, build_cache_control_value, build_etag_value,
    headers as cache_header, if_none_match_hits, parse_if_none_match_entries,
    supports_http_revalidation,
};
use self::parameter_parsing::{parse_path_segment_value, parse_query_or_header_value};
use self::response_mapping::interpret_agent_response;
use super::RichRequest;
use super::error::RequestHandlerError;
use super::model::{ResponseBody, RichRouteSecurity};
use super::route_resolver::ResolvedRouteEntry;
use super::{ParsedRequestBody, RouteExecutionResult};
use crate::service::worker::WorkerService;
use anyhow::anyhow;
use golem_common::model::OplogIndex;
use golem_common::model::Timestamp;
use golem_common::model::agent::{AgentMode, AgentTarget, ParsedAgentKey, ReadOnlyConfig};
use golem_common::model::caller::{Caller, Claims, Pair, Principal, UserPrincipal};
use golem_common::model::{AgentFingerprint, AgentId};
use golem_common::schema::unstructured::wrap_unstructured_inline_for_schema;
use golem_common::schema::{BinaryValuePayload, SchemaValue, TextValuePayload, TypedSchemaValue};
use golem_service_base::custom_api::{
    CallAgentBehaviour, ConstructorParameter, MethodParameter, PhantomSelection, RequestBodySchema,
};
use golem_service_base::model::auth::AuthCtx;
use http::{Method, StatusCode};
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
        let phantom_id = resolve_phantom_id(
            request,
            resolved_route,
            &behaviour.phantom_selection,
            |phantom| (phantom && behaviour.agent_mode == AgentMode::Durable).then(Uuid::new_v4),
        )?;

        let agent_id = Self::build_agent_id(
            resolved_route,
            behaviour.component_id,
            &behaviour.agent_type,
            &behaviour.constructor_input,
            &behaviour.constructor_parameters,
            phantom_id,
        )?;

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

        let parsed_body = request.parse_request_body(&behaviour.body).await?;

        let method_params =
            self.resolve_method_arguments(resolved_route, request, behaviour, parsed_body)?;

        debug!("Invoking agent {agent_id}");

        let method_params_value = SchemaValue::Record {
            fields: method_params,
        };

        let proto_method_parameters: golem_schema::proto::golem::schema::SchemaValue =
            method_params_value.try_into().map_err(|error| {
                anyhow!("method parameters cannot cross the worker boundary: {error}")
            })?;

        let invocation_context = Some(golem_api_grpc::proto::golem::worker::InvocationContext {
            parent: None,
            env: Default::default(),
            tracing: Some(request.invocation_context().into()),
        });

        let caller = caller_from_request(request)?;
        debug!(kind = %caller.original.kind(), "Using caller for invocation");
        let proto_caller: golem_api_grpc::proto::golem::component::Caller = caller.clone().into();

        let agent_response = self
            .worker_service
            .invoke_agent(
                &AgentTarget::Current(agent_id.clone()),
                Some(behaviour.method_name.clone()),
                Some(proto_method_parameters),
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                None,
                golem_common::model::InvocationRequestKey::external(None, None),
                invocation_context,
                false,
                golem_common::model::agent::InvocationFreshnessDisposition::MayExist,
                Vec::new(),
                AuthCtx::System,
                proto_caller,
                Some(resolved_route.route.environment_id),
                None,
                None,
            )
            .await?;

        debug!("Received agent response: {agent_response:?}");

        // The executor returns the oplog index of the agent right after the
        // invocation, paired with the per-instance agent fingerprint of the
        // worker that produced it. Using this index+fingerprint pair as the
        // ETag validator avoids TOCTOU: because the cached entry stores the
        // same pair, a subsequent cache hit produces the same ETag; any
        // non-read-only write between two reads bumps the cache epoch and
        // forces a new invocation whose recorded index will be strictly
        // greater; and the fingerprint ensures that a deleted-then-recreated
        // agent that coincidentally lands at the same oplog index does not
        // collide with a previously-cached client-side ETag.
        let oplog_index = agent_response.oplog_index;
        let agent_fingerprint = agent_response.agent_fingerprint;

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
                oplog_index,
                agent_fingerprint,
                read_only,
                behaviour,
                resolved_route,
                &caller,
                request.headers(),
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
    /// `(fingerprint, oplog index)` pair, we short-circuit with
    /// `304 Not Modified` and do not touch the executor at all.
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

        let Some((current_fingerprint, current_oplog)) =
            self.fetch_current_validator(agent_id).await?
        else {
            return Ok(None);
        };

        // The validator names the caller it was issued to, so only the same caller's request
        // revalidates: a cache may send the ETag of another caller's stored response.
        let caller = caller_from_request(request)?;
        if !if_none_match_hits(
            &entries,
            agent_id,
            current_fingerprint,
            &caller,
            current_oplog,
        ) {
            return Ok(None);
        }

        let mut headers = http::HeaderMap::new();
        add_read_only_cache_headers(
            &mut headers,
            agent_id,
            Some(current_oplog),
            Some(current_fingerprint),
            read_only,
            behaviour,
            resolved_route,
            &caller,
            request.headers(),
        );

        Ok(Some(RouteExecutionResult {
            status: StatusCode::NOT_MODIFIED,
            headers,
            body: ResponseBody::NoBody,
        }))
    }

    /// Fetch the agent's current `(fingerprint, last_oplog_index)` pair, used
    /// as the strong-validator value for `If-None-Match` revalidation. The
    /// pair must come from the same metadata snapshot so the fingerprint and
    /// the oplog index describe the same agent instance.
    async fn fetch_current_validator(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<(AgentFingerprint, OplogIndex)>, RequestHandlerError> {
        match self
            .worker_service
            .get_metadata(agent_id, AuthCtx::System)
            .await
        {
            Ok(metadata) => Ok(Some((metadata.fingerprint, metadata.last_oplog_index))),
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

    pub(super) fn build_agent_id(
        resolved_route: &ResolvedRouteEntry,
        component_id: golem_common::model::component::ComponentId,
        agent_type: &golem_common::model::agent::AgentTypeName,
        constructor_input: &golem_service_base::custom_api::CompiledInputSchema,
        constructor_parameters: &[ConstructorParameter],
        phantom_id: Option<Uuid>,
    ) -> Result<AgentId, RequestHandlerError> {
        let mut fields = Vec::with_capacity(constructor_parameters.len());

        for param in constructor_parameters {
            match param {
                ConstructorParameter::Path {
                    path_segment_index,
                    parameter_type,
                } => {
                    let raw = resolved_route.captured_path_parameters
                        [usize::from(*path_segment_index)]
                    .clone();

                    fields.push(parse_path_segment_value(raw, parameter_type)?);
                }
            }
        }

        // The agent-id parameters travel as a self-contained `TypedSchemaValue`:
        // the constructor's compiled `SchemaGraph` paired with the positional
        // record of the (user-supplied) constructor parameters.
        let parameters = TypedSchemaValue::new(
            constructor_input.graph.clone(),
            SchemaValue::Record { fields },
        );

        let agent_id = ParsedAgentKey::try_new(agent_type.clone(), parameters, phantom_id)
            .map_err(|e| RequestHandlerError::AgentResponseTypeMismatch { error: e })?;

        Ok(AgentId {
            component_id,
            agent_id: agent_id.to_string(),
        })
    }

    pub(super) fn resolve_method_arguments(
        &self,
        resolved_route: &ResolvedRouteEntry,
        request: &RichRequest,
        behaviour: &CallAgentBehaviour,
        mut body: ParsedRequestBody,
    ) -> Result<Vec<SchemaValue>, RequestHandlerError> {
        let query_params = request.query_params();
        let headers = request.headers();

        // The producer emits `method_parameters` in user-supplied input
        // declaration order, so iterating them in order builds the positional
        // `SchemaValue::Record` the executor validates against (which excludes
        // auto-injected fields like the caller).
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
                    ParsedRequestBody::JsonBody(SchemaValue::Record { fields }) => fields
                        .get(usize::from(*field_index))
                        .cloned()
                        .ok_or_else(|| {
                            RequestHandlerError::invariant_violated(
                                "JSON body field index out of range for parsed body",
                            )
                        })?,

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

                        let raw = SchemaValue::Binary(BinaryValuePayload {
                            bytes: binary_source.data,
                            mime_type: binary_source.mime_type,
                        });
                        // The field schema is either the canonical unstructured
                        // wrapper (raw body -> `inline` case; DA: url-referenced
                        // request bodies are not accepted) or a bare `Binary`
                        // rich scalar (raw value as-is).
                        wrap_unstructured_body_value(&behaviour.body, raw)?
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

                        let raw = SchemaValue::Text(TextValuePayload {
                            text: text_source.data,
                            language: text_source
                                .text_type
                                .map(|text_type| text_type.language_code),
                        });
                        // The field schema is either the canonical unstructured
                        // wrapper (raw body -> `inline` case; DA: url-referenced
                        // request bodies are not accepted) or a bare `Text` rich
                        // scalar (raw value as-is).
                        wrap_unstructured_body_value(&behaviour.body, raw)?
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

/// Build the method-input value for a raw unstructured request body, wrapping
/// the decoded `Text` / `Binary` value in the canonical `inline` case when the
/// bound field schema is an unstructured `variant { inline, url }` wrapper, or
/// passing it through unchanged for a bare `Text` / `Binary` rich scalar.
fn wrap_unstructured_body_value(
    body_schema: &RequestBodySchema,
    raw: SchemaValue,
) -> Result<SchemaValue, RequestHandlerError> {
    let expected = match body_schema {
        RequestBodySchema::BinaryBody { expected } | RequestBodySchema::TextBody { expected } => {
            expected
        }
        _ => {
            return Err(RequestHandlerError::invariant_violated(
                "Unstructured body parameter used but body schema is not a text/binary body",
            ));
        }
    };
    wrap_unstructured_inline_for_schema(&expected.graph, &expected.graph.root, raw)
        .map_err(|err| anyhow!("Failed to build unstructured body value: {err}").into())
}

/// HTTP methods for which the worker-service emits read-only cache headers
/// (`ETag`, `Cache-Control`) and serves `304 Not Modified` from
/// `If-None-Match`.
///
/// This set MUST be kept in sync with
/// `golem_registry_service::services::deployment::route_compilation::collect_read_only_warnings`,
/// which warns the user at deploy time when a read-only `AgentMethod` is bound
/// to an HTTP verb that is **not** in this whitelist (because the worker-
/// service will then never emit cache headers for that binding).
fn is_cacheable_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
}

/// Insert the cache headers for a read-only `AgentMethod` response into the
/// given header map: `Cache-Control`, optionally `ETag` (when the cache policy
/// supports HTTP revalidation), and `Vary` for every header that influences
/// the response (the headers that carry a credential, for public and private
/// answers alike, plus any header-bound method parameters).
#[allow(clippy::too_many_arguments)]
fn add_read_only_cache_headers(
    headers: &mut http::HeaderMap,
    agent_id: &AgentId,
    oplog_index: Option<OplogIndex>,
    agent_fingerprint: Option<AgentFingerprint>,
    read_only: &ReadOnlyConfig,
    behaviour: &CallAgentBehaviour,
    resolved_route: &ResolvedRouteEntry,
    caller: &Caller,
    request_headers: &http::HeaderMap,
) {
    let session_header = session_header_name(&resolved_route.route.security);
    let visibility = CacheVisibility::for_request(caller, request_headers, session_header);
    let issues_token = headers.contains_key(http::header::SET_COOKIE);
    headers.insert(
        cache_header::CACHE_CONTROL,
        http::HeaderValue::from_str(&build_cache_control_value(
            read_only,
            visibility,
            issues_token,
        ))
        .unwrap(),
    );

    // `CachePolicy::NoCache` explicitly opts out of HTTP caching: no ETag, and
    // therefore no 304 path. Otherwise emit an ETag built from the supplied
    // `(agent_fingerprint, oplog_index)` pair: on the `200` path both come
    // from the executor-reported post-commit (or cache-entry) values for the
    // read-only invocation; on the `304` path both come from the same
    // metadata snapshot observed before short-circuiting. Skip the ETag
    // entirely if either half of the validator is unavailable (e.g. metadata
    // fetch failed, or the executor did not return one — for example because
    // it is a legacy build that predates the fingerprint field).
    if supports_http_revalidation(read_only)
        && let (Some(idx), Some(fp)) = (oplog_index, agent_fingerprint)
    {
        headers.insert(
            cache_header::ETAG,
            http::HeaderValue::from_str(&build_etag_value(agent_id, fp, caller, idx)).unwrap(),
        );
    }

    // Every answer varies on every header that can carry a credential: a public answer may be
    // served by a shared cache to any request, and a private answer may be kept by a browser
    // cache that another session of the same browser then asks, so neither is selected for a
    // request with a different credential. The headers that method parameters are bound to vary
    // the response as well.
    for name in ["Authorization", "Cookie", "Golem-Token"]
        .into_iter()
        .chain(session_header)
    {
        add_vary_header(headers, name);
    }
    for param in &behaviour.method_parameters {
        if let MethodParameter::Header { header_name, .. } = param {
            add_vary_header(headers, header_name);
        }
    }
}

/// The header that carries the session of a `SessionFromHeader` route.
fn session_header_name(security: &RichRouteSecurity) -> Option<&str> {
    match security {
        RichRouteSecurity::SessionFromHeader(s) => Some(s.header_name.as_str()),
        RichRouteSecurity::None
        | RichRouteSecurity::SecurityScheme(_)
        | RichRouteSecurity::Unavailable => None,
    }
}

pub(super) fn resolve_phantom_id(
    request: &RichRequest,
    resolved_route: &ResolvedRouteEntry,
    selection: &PhantomSelection,
    policy: impl FnOnce(bool) -> Option<Uuid>,
) -> Result<Option<Uuid>, RequestHandlerError> {
    let value = match selection {
        PhantomSelection::Policy { phantom } => return Ok(policy(*phantom)),
        PhantomSelection::Original => return Ok(None),
        PhantomSelection::Path { index } => {
            resolved_route.captured_path_parameters[usize::from(*index)].as_str()
        }
        PhantomSelection::Query { name, optional } => {
            match request.query_params().get(name).map(Vec::as_slice) {
                Some([value]) => value.as_str(),
                None if *optional => return Ok(None),
                None | Some([]) => {
                    return Err(RequestHandlerError::MissingValue {
                        expected: "phantom UUID query selector",
                    });
                }
                _ => {
                    return Err(RequestHandlerError::TooManyValues {
                        expected: "phantom UUID query selector",
                    });
                }
            }
        }
    };
    Uuid::parse_str(value)
        .map(Some)
        .map_err(|_| RequestHandlerError::ValueParsingFailed {
            value: value.into(),
            expected: "UUID",
        })
}

/// The caller of a request: the user of its authenticated session, or anonymous. The pair is the
/// session's issuer and subject exactly as written, never the email or the name. The claims are
/// the session's claim set, kept as the JSON text that was serialized.
pub(super) fn caller_from_request(request: &RichRequest) -> Result<Caller, RequestHandlerError> {
    match request.authenticated_session() {
        Some(session) => {
            let claims_json = serde_json::to_string(&session.claims)
                .map_err(|e| anyhow!("CoreIdTokenClaims serialization error: {e}"))?;
            let claims = Claims::from_json(claims_json)
                .map_err(|e| anyhow!("the session claims are not a valid claim set: {e}"))?;
            let authenticated_at = session
                .claims
                .auth_time()
                .unwrap_or_else(|| session.claims.issue_time());
            let authenticated_at = Timestamp::from(
                u64::try_from(authenticated_at.timestamp_millis())
                    .map_err(|_| anyhow!("the session claims have a time before 1970"))?,
            );
            Ok(Caller::entry(Principal::User(UserPrincipal {
                pair: Pair::new(session.issuer.clone(), session.subject.clone()),
                claims,
                authenticated_at,
            })))
        }
        None => Ok(Caller::anonymous()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::schema::{InputSchema, SchemaGraph, SchemaType};
    use golem_service_base::custom_api::{CompiledInputSchema, PathSegmentType};
    use test_r::test;

    fn oidc_session(issuer: &str, subject: &str) -> crate::custom_api::model::OidcSession {
        use chrono::{TimeDelta, Utc};
        use openidconnect::{
            EmptyAdditionalClaims, IdTokenClaims, IssuerUrl, StandardClaims, SubjectIdentifier,
        };

        let issued_at = Utc::now();
        crate::custom_api::model::OidcSession {
            subject: subject.to_string(),
            issuer: issuer.to_string(),
            email: Some("ana@acme.example".to_string()),
            name: Some("Ana".to_string()),
            email_verified: Some(true),
            given_name: None,
            family_name: None,
            picture: None,
            preferred_username: Some("ana".to_string()),
            claims: IdTokenClaims::new(
                IssuerUrl::new(issuer.to_string()).unwrap(),
                Vec::new(),
                issued_at + TimeDelta::hours(1),
                issued_at,
                StandardClaims::new(SubjectIdentifier::new(subject.to_string())),
                EmptyAdditionalClaims {},
            ),
            scopes: Default::default(),
            expires_at: issued_at + TimeDelta::hours(1),
        }
    }

    #[test]
    fn caller_from_request_builds_the_user_pair_from_issuer_and_sub() {
        let mut request = RichRequest::new(poem::Request::default());
        assert_eq!(caller_from_request(&request).unwrap(), Caller::anonymous());

        let session = oidc_session("https://login.acme.example/", "Ana.Example");
        let claims_text = serde_json::to_string(&session.claims).unwrap();
        request.set_authenticated_session(session);

        let caller = caller_from_request(&request).unwrap();
        assert!(caller.tokens.is_empty());
        assert_eq!(
            caller.original.kind(),
            golem_common::model::caller::Kind::User
        );
        // Both roles hold the same user.
        assert!(caller.original.equals(&caller.immediate));
        // The pair is the issuer and subject as written, never the email or the name.
        assert_eq!(
            caller.original.pair().unwrap().to_string(),
            "(\"https://login.acme.example/\",\"Ana.Example\")"
        );
        let Principal::User(user) = &caller.original else {
            panic!("expected a user");
        };
        // The claims text is the exact serialization of the session's claims.
        assert_eq!(user.claims.as_json(), claims_text);
        assert!(!format!("{:?}", caller.original.pair()).contains("ana@acme.example"));
    }

    #[test]
    async fn explicit_phantom_selection_validates_decoded_queries_and_capture_ordinals() {
        let p = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let q = "8badf00d-1234-4567-89ab-0123456789ab";
        let resolver = test_resolver(vec![test_route(
            1,
            "/base/{decision}/{instance}/literal/{tenant}",
            Some("GET"),
            "typed",
        )]);
        let route_request = poem::Request::builder()
            .uri(format!("/base/d7/{p}/literal/west").parse().unwrap())
            .header("host", "example.com")
            .finish();
        let mut resolved = resolver
            .resolve_matching_route(&route_request)
            .await
            .unwrap();
        let path = PhantomSelection::Path { index: 1.into() };
        assert_eq!(
            resolve_phantom_id(
                &RichRequest::new(route_request),
                &resolved,
                &path,
                |_| panic!("explicit selection must not use policy")
            )
            .unwrap(),
            Some(p)
        );
        for optional in [false, true] {
            let selection = PhantomSelection::Query {
                name: "instance".into(),
                optional,
            };
            for query in [
                format!("instance={p}"),
                format!("instance={}", p.to_string().to_uppercase()),
                format!("%69nstance={}", p.to_string().replace('-', "%2D")),
                format!("other=one&instance={p}&other=two"),
            ] {
                let request = RichRequest::new(
                    poem::Request::builder()
                        .uri(format!("/base?{query}").parse().unwrap())
                        .finish(),
                );
                assert_eq!(
                    resolve_phantom_id(&request, &resolved, &selection, |_| panic!(
                        "explicit selection must not use policy"
                    ))
                    .unwrap(),
                    Some(p)
                );
            }
            for query in [
                "instance=".into(),
                "instance=invalid".into(),
                format!("instance={p}&instance={p}"),
                format!("instance={p}&instance={q}"),
                format!("instance={p}&%69nstance={p}"),
            ] {
                let request = RichRequest::new(
                    poem::Request::builder()
                        .uri(format!("/base?{query}").parse().unwrap())
                        .finish(),
                );
                assert!(
                    resolve_phantom_id(&request, &resolved, &selection, |_| panic!(
                        "invalid explicit selection must not use policy"
                    ))
                    .is_err()
                );
            }
            let request = RichRequest::new(
                poem::Request::builder()
                    .uri("/base".parse().unwrap())
                    .finish(),
            );
            let result = resolve_phantom_id(&request, &resolved, &selection, |_| {
                panic!("optional omission must not use policy")
            });
            if optional {
                assert_eq!(result.unwrap(), None);
            } else {
                assert!(result.is_err());
            }
        }
        let request = RichRequest::new(poem::Request::builder().finish());
        assert_eq!(
            resolve_phantom_id(
                &request,
                &resolved,
                &PhantomSelection::Original,
                |_| panic!("original must not use policy")
            )
            .unwrap(),
            None
        );
        for phantom in [false, true] {
            assert_eq!(
                resolve_phantom_id(
                    &request,
                    &resolved,
                    &PhantomSelection::Policy { phantom },
                    |flag| {
                        assert_eq!(flag, phantom);
                        Some(p)
                    }
                )
                .unwrap(),
                Some(p)
            );
        }
        let constructor = CompiledInputSchema {
            graph: SchemaGraph::anonymous(SchemaType::record(vec![
                golem_common::schema::NamedFieldType {
                    name: "decision".into(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                },
                golem_common::schema::NamedFieldType {
                    name: "tenant".into(),
                    body: SchemaType::string(),
                    metadata: Default::default(),
                },
            ])),
            input_schema: InputSchema::Parameters(vec![]),
        };
        let bindings = vec![
            ConstructorParameter::Path {
                path_segment_index: 0.into(),
                parameter_type: PathSegmentType::Str,
            },
            ConstructorParameter::Path {
                path_segment_index: 2.into(),
                parameter_type: PathSegmentType::Str,
            },
        ];
        let component_id = golem_common::model::component::ComponentId::new();
        let agent_type = golem_common::model::agent::AgentTypeName("selected".into());
        let id = CallAgentHandler::build_agent_id(
            &resolved,
            component_id,
            &agent_type,
            &constructor,
            &bindings,
            Some(p),
        )
        .unwrap();
        assert_eq!(id.agent_id, format!("selected(\"d7\",\"west\")[{p}]"));
        resolved.captured_path_parameters[0] = "d9".into();
        let other = CallAgentHandler::build_agent_id(
            &resolved,
            component_id,
            &agent_type,
            &constructor,
            &bindings,
            Some(p),
        )
        .unwrap();
        assert_eq!(other.agent_id, format!("selected(\"d9\",\"west\")[{p}]"));
        assert_ne!(id, other);
        let other_component = CallAgentHandler::build_agent_id(
            &resolved,
            golem_common::model::component::ComponentId::new(),
            &agent_type,
            &constructor,
            &bindings,
            Some(p),
        )
        .unwrap();
        assert_eq!(other.agent_id, other_component.agent_id);
        assert_ne!(other, other_component);
    }

    /// Ana signs in on a `SessionFromHeader` route and reads a read-only method; then Ben uses the
    /// same browser. RFC 9111 4.1: a stored response is selected for a later request only when
    /// every request header that `Vary` names has the same value, so a private response must
    /// vary on the session header, and the validator of a response is never accepted for
    /// another session.
    #[test]
    async fn a_private_response_is_never_reused_for_another_session() {
        use golem_common::model::agent::{CachePolicy, CachePolicyTtl};
        use golem_service_base::custom_api::{RouteSecurity, SessionFromHeaderRouteSecurity};

        let mut route = test_route(1, "/profile", Some("GET"), "typed");
        route.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
            header_name: "X-Session".into(),
        });
        let resolver = test_resolver(vec![route]);
        let route_request = poem::Request::builder()
            .uri("/profile".parse().unwrap())
            .header("host", "example.com")
            .finish();
        let resolved = resolver
            .resolve_matching_route(&route_request)
            .await
            .unwrap();
        let crate::custom_api::model::RichRouteBehaviour::CallAgent(behaviour) =
            &resolved.route.behavior
        else {
            panic!("expected an agent route");
        };
        let agent_id = AgentId {
            component_id: behaviour.component_id,
            agent_id: "agent1()".to_string(),
        };
        let fingerprint = AgentFingerprint(Uuid::nil());
        let index = OplogIndex::from_u64(7);
        let user = |subject: &str| {
            Caller::entry(Principal::User(UserPrincipal {
                pair: Pair::new(
                    "https://login.acme.example/".to_string(),
                    subject.to_string(),
                ),
                claims: Claims::from_json("{}".to_string()).unwrap(),
                authenticated_at: Timestamp::from(0u64),
            }))
        };
        let session = |value: &'static str| {
            let mut headers = http::HeaderMap::new();
            headers.insert("x-session", http::HeaderValue::from_static(value));
            headers
        };

        let mut failures = Vec::new();
        for cache_policy in [
            CachePolicy::UntilWrite(golem_schema::Empty {}),
            CachePolicy::Ttl(CachePolicyTtl {
                duration_nanos: 60_000_000_000,
            }),
        ] {
            let read_only = ReadOnlyConfig { cache_policy };
            let ana_request = session("session-of-ana");
            let ben_request = session("session-of-ben");
            let mut ana_response = http::HeaderMap::new();
            add_read_only_cache_headers(
                &mut ana_response,
                &agent_id,
                Some(index),
                Some(fingerprint),
                &read_only,
                behaviour,
                &resolved,
                &user("ana"),
                &ana_request,
            );
            let vary = ana_response
                .get(http::header::VARY)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            let reusable_for_ben = vary
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .all(|name| ana_request.get(name) == ben_request.get(name));
            // Ben's conditional request with Ana's ETag: the validator names Ana, so the worker
            // service does not answer 304 for Ben, and the agent runs for him.
            let etag = ana_response
                .get(http::header::ETAG)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let ben_gets_304 = if_none_match_hits(
                &parse_if_none_match_entries(&etag),
                &agent_id,
                fingerprint,
                &user("ben"),
                index,
            );
            println!(
                "{:?}: cache-control={:?} vary={vary:?} ana's stored response reusable for ben={reusable_for_ben} ben's revalidation gets 304={ben_gets_304}",
                read_only.cache_policy,
                ana_response.get(http::header::CACHE_CONTROL),
            );
            if reusable_for_ben {
                failures.push(format!(
                    "{:?}: Ana's stored response",
                    read_only.cache_policy
                ));
            }
            // A cache may send the ETag of a stored response that Vary does not select, and
            // reuse that response when the answer is 304.
            if ben_gets_304 {
                failures.push(format!(
                    "{:?}: Ben's revalidation with Ana's ETag",
                    read_only.cache_policy
                ));
            }

            // A public answer (anonymous, no credential) and a private one name the same
            // credential headers.
            let mut public_response = http::HeaderMap::new();
            add_read_only_cache_headers(
                &mut public_response,
                &agent_id,
                Some(index),
                Some(fingerprint),
                &read_only,
                behaviour,
                &resolved,
                &Caller::anonymous(),
                &http::HeaderMap::new(),
            );
            for response in [&ana_response, &public_response] {
                let vary = response
                    .get(http::header::VARY)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                for name in ["authorization", "cookie", "golem-token", "x-session"] {
                    assert!(
                        vary.split(',').any(|entry| entry.trim() == name),
                        "{:?}: Vary {vary:?} must name {name}",
                        read_only.cache_policy
                    );
                }
            }
        }
        assert!(
            failures.is_empty(),
            "Ana's private response can be served to Ben by the browser cache for {failures:?}"
        );
    }

    /// The route, the agent and the validator inputs of one read-only response, for the ETag
    /// tests below.
    async fn read_only_profile_route() -> (
        crate::custom_api::route_resolver::ResolvedRouteEntry,
        AgentId,
        AgentFingerprint,
        OplogIndex,
        ReadOnlyConfig,
    ) {
        use golem_common::model::agent::{CachePolicy, CachePolicyTtl};
        use golem_service_base::custom_api::{RouteSecurity, SessionFromHeaderRouteSecurity};

        let mut route = test_route(1, "/profile", Some("GET"), "typed");
        route.security = RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
            header_name: "X-Session".into(),
        });
        let resolver = test_resolver(vec![route]);
        let request = poem::Request::builder()
            .uri("/profile".parse().unwrap())
            .header("host", "example.com")
            .finish();
        let resolved = resolver.resolve_matching_route(&request).await.unwrap();
        let crate::custom_api::model::RichRouteBehaviour::CallAgent(behaviour) =
            &resolved.route.behavior
        else {
            panic!("expected an agent route");
        };
        let agent_id = AgentId {
            component_id: behaviour.component_id,
            agent_id: "agent1()".to_string(),
        };
        let read_only = ReadOnlyConfig {
            cache_policy: CachePolicy::Ttl(CachePolicyTtl {
                duration_nanos: 60_000_000_000,
            }),
        };
        (
            resolved,
            agent_id,
            AgentFingerprint(Uuid::nil()),
            OplogIndex::from_u64(7),
            read_only,
        )
    }

    fn signed_in_user(subject: &str) -> Caller {
        Caller::entry(Principal::User(UserPrincipal {
            pair: Pair::new(
                "https://login.acme.example/".to_string(),
                subject.to_string(),
            ),
            claims: Claims::from_json("{}".to_string()).unwrap(),
            authenticated_at: Timestamp::from(0u64),
        }))
    }

    /// The ETag that a read-only response to `caller` carries.
    fn etag_for(
        resolved: &crate::custom_api::route_resolver::ResolvedRouteEntry,
        agent_id: &AgentId,
        fingerprint: AgentFingerprint,
        index: OplogIndex,
        read_only: &ReadOnlyConfig,
        caller: &Caller,
    ) -> String {
        let crate::custom_api::model::RichRouteBehaviour::CallAgent(behaviour) =
            &resolved.route.behavior
        else {
            panic!("expected an agent route");
        };
        let mut response = http::HeaderMap::new();
        add_read_only_cache_headers(
            &mut response,
            agent_id,
            Some(index),
            Some(fingerprint),
            read_only,
            behaviour,
            resolved,
            caller,
            &http::HeaderMap::new(),
        );
        response[http::header::ETAG].to_str().unwrap().to_string()
    }

    /// Ana reads a read-only method and gets an ETag. Ben, another session, then sends a
    /// conditional request with Ana's ETag, as a cache may do for a stored response that `Vary`
    /// does not select. A read-only call appends nothing to the oplog, so the index is the same:
    /// the validator must still not match, or the 304 lets the cache show Ana's body to Ben.
    #[test]
    async fn another_callers_etag_never_revalidates_a_request() {
        let (resolved, agent_id, fingerprint, index, read_only) = read_only_profile_route().await;
        let ana_etag = etag_for(
            &resolved,
            &agent_id,
            fingerprint,
            index,
            &read_only,
            &signed_in_user("ana"),
        );
        let ben_gets_304 = if_none_match_hits(
            &parse_if_none_match_entries(&ana_etag),
            &agent_id,
            fingerprint,
            &signed_in_user("ben"),
            index,
        );
        assert!(
            !ben_gets_304,
            "Ana's ETag {ana_etag} revalidates Ben's request: 304 for another caller"
        );
    }

    /// The ETag of a response still revalidates the same caller's next request while the agent's
    /// oplog has not moved.
    #[test]
    async fn the_same_callers_etag_still_revalidates() {
        let (resolved, agent_id, fingerprint, index, read_only) = read_only_profile_route().await;
        let ana_etag = etag_for(
            &resolved,
            &agent_id,
            fingerprint,
            index,
            &read_only,
            &signed_in_user("ana"),
        );
        assert!(if_none_match_hits(
            &parse_if_none_match_entries(&ana_etag),
            &agent_id,
            fingerprint,
            &signed_in_user("ana"),
            index,
        ));
    }
}
