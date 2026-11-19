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
    build_etag_value, headers as cache_header, if_none_match_hits, parse_if_none_match_entries,
    supports_http_revalidation,
};
pub(super) use self::parameter_parsing::validate_query_parameters;
use self::parameter_parsing::{
    parse_path_segment_value, parse_query_or_header_value, parse_query_parameter,
};
use self::response_mapping::interpret_agent_response;
use super::RichRequest;
use super::cache_headers::{CacheRouteKind, add_vary_headers, apply_response_cache_headers};
use super::error::RequestHandlerError;
use super::model::ResponseBody;
use super::route_resolver::ResolvedRouteEntry;
use super::{ParsedRequestBody, RouteExecutionResult};
use crate::service::worker::{AgentBinding, WorkerService};
use anyhow::anyhow;
use golem_common::model::OplogIndex;
use golem_common::model::agent::{
    InvocationFreshnessDisposition, OidcPrincipal, ParsedAgentKey, Principal, ReadOnlyConfig,
};
use golem_common::model::{AgentFingerprint, AgentId, InvocationRequestKey, OwnedAgentId};
use golem_common::schema::unstructured::wrap_unstructured_inline_for_schema;
use golem_common::schema::validation::validate_value;
use golem_common::schema::{
    BinaryValuePayload, FieldSource, SchemaType, SchemaValue, TextValuePayload, TypedSchemaValue,
};
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
        validate_query_parameters(request, behaviour, &[])?;
        let binding = route_agent_binding(
            request,
            resolved_route,
            &behaviour.phantom_selection,
            |phantom| {
                if phantom {
                    AgentBinding::Fresh
                } else {
                    AgentBinding::Constructor
                }
            },
        )?;
        let constructor = Self::route_constructor(
            resolved_route,
            &behaviour.agent_type,
            &behaviour.constructor_input,
            &behaviour.constructor_parameters,
        )?;
        let request_key = InvocationRequestKey::external(None, None);
        let (target, component, freshness_disposition) = self
            .worker_service
            .bound_invocation_target(
                behaviour.component_id,
                &constructor,
                binding,
                &request_key,
                &behaviour.method_name,
                &AuthCtx::System,
            )
            .await?;
        let agent_id = OwnedAgentId::new(resolved_route.route.environment_id, &target.agent_id);

        let request_method = request.underlying.method().clone();

        // For read-only methods bound to GET/HEAD routes with a revalidatable
        // cache policy, try to serve a `304 Not Modified` without invoking the
        // executor when the client's `If-None-Match` value matches the agent's
        // current oplog index. `get_metadata` is safe here because the
        // executor implementation reads cached metadata / persisted oplog
        // state and does **not** wake or load the worker.
        if let Some(read_only) = behaviour.read_only.as_ref()
            && freshness_disposition == InvocationFreshnessDisposition::MayExist
            && is_cacheable_method(&request_method)
            && supports_http_revalidation(read_only)
            && let Some(not_modified) = self
                .try_handle_if_none_match(request, behaviour, &agent_id, read_only)
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

        let principal = principal_from_request(request)?;
        debug!("Using principal for invocation: {principal:?}");
        let proto_principal: golem_api_grpc::proto::golem::component::Principal = principal.into();

        let agent_response = self
            .worker_service
            .dispatch_agent_invocation(
                &component,
                &target,
                Some(behaviour.method_name.clone()),
                Some(proto_method_parameters),
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                None,
                request_key,
                invocation_context,
                freshness_disposition,
                Vec::new(),
                resolved_route.route.environment_id,
                component.account_id,
                AuthCtx::System,
                proto_principal,
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
                request.authenticated_session().is_some(),
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
        agent_id: &OwnedAgentId,
        read_only: &ReadOnlyConfig,
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

        if !if_none_match_hits(&entries, agent_id, current_fingerprint, current_oplog) {
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
            request.authenticated_session().is_some(),
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
        agent_id: &OwnedAgentId,
    ) -> Result<Option<(AgentFingerprint, OplogIndex)>, RequestHandlerError> {
        match self
            .worker_service
            .get_metadata(agent_id, None, AuthCtx::System)
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

    /// The constructor of the agent of a route, from the path segments it binds.
    pub(super) fn route_constructor(
        resolved_route: &ResolvedRouteEntry,
        agent_type: &golem_common::model::agent::AgentTypeName,
        constructor_input: &golem_service_base::custom_api::CompiledInputSchema,
        constructor_parameters: &[ConstructorParameter],
    ) -> Result<ParsedAgentKey, RequestHandlerError> {
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

        // The constructor parameters travel as a self-contained `TypedSchemaValue`: the
        // constructor's compiled `SchemaGraph` paired with the positional record of the
        // (user-supplied) constructor parameters.
        let parameters = TypedSchemaValue::new(
            constructor_input.graph.clone(),
            SchemaValue::Record { fields },
        );

        ParsedAgentKey::try_new(agent_type.clone(), parameters)
            .map_err(|e| RequestHandlerError::AgentResponseTypeMismatch { error: e })
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
        // auto-injected fields like the principal).
        let mut values = Vec::with_capacity(behaviour.method_parameters.len());
        let mut input_fields = behaviour
            .method_input
            .input_schema
            .fields()
            .iter()
            .filter(|field| matches!(field.source, FieldSource::UserSupplied))
            .filter(|field| {
                !matches!(
                    behaviour.method_input.graph.resolve_ref(&field.schema),
                    Ok(SchemaType::Stream { .. })
                )
            });
        for param in &behaviour.method_parameters {
            let field = input_fields.next().ok_or_else(|| {
                RequestHandlerError::invariant_violated("Method parameter has no input field")
            })?;
            let invalid =
                |error: RequestHandlerError| RequestHandlerError::ParameterParsingFailed {
                    name: field.name.clone(),
                    error: Box::new(error),
                };
            let value = match param {
                MethodParameter::Path {
                    path_segment_index,
                    parameter_type,
                } => {
                    let raw = resolved_route.captured_path_parameters
                        [usize::from(*path_segment_index)]
                    .clone();

                    parse_path_segment_value(raw, parameter_type).map_err(&invalid)?
                }

                MethodParameter::Query {
                    query_parameter_name,
                    parameter_type,
                } => parse_query_parameter(query_parameter_name, query_params, parameter_type)
                    .map_err(&invalid)?,

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

                    parse_query_or_header_value(&vals, parameter_type).map_err(&invalid)?
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
            if matches!(
                param,
                MethodParameter::Path { .. }
                    | MethodParameter::Query { .. }
                    | MethodParameter::Header { .. }
            ) {
                validate_value(&behaviour.method_input.graph, &field.schema, &value).map_err(
                    |errors| RequestHandlerError::InvalidParameter {
                        name: field.name.clone(),
                        error: errors
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                    },
                )?;
            }

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
/// supports HTTP revalidation), and `Vary` for header-bound method parameters.
/// Credential-related Vary headers are also applied at the response boundary.
fn add_read_only_cache_headers(
    headers: &mut http::HeaderMap,
    agent_id: &OwnedAgentId,
    oplog_index: Option<OplogIndex>,
    agent_fingerprint: Option<AgentFingerprint>,
    read_only: &ReadOnlyConfig,
    behaviour: &CallAgentBehaviour,
    request_authenticated: bool,
) {
    apply_response_cache_headers(
        CacheRouteKind::Typed(Some(&read_only.cache_policy)),
        headers,
        request_authenticated,
        false,
        false,
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
            http::HeaderValue::from_str(&build_etag_value(agent_id, fp, idx)).unwrap(),
        );
    }

    for param in &behaviour.method_parameters {
        if let MethodParameter::Header { header_name, .. } = param {
            add_vary_headers(headers, &[header_name]);
        }
    }
}

/// The agent that a route selects beside its constructor. An agent ID that the request
/// selects with a path segment or a query parameter names an existing agent; `policy` gives
/// the binding of a route that selects none.
pub(super) fn route_agent_binding(
    request: &RichRequest,
    resolved_route: &ResolvedRouteEntry,
    selection: &PhantomSelection,
    policy: impl FnOnce(bool) -> AgentBinding,
) -> Result<AgentBinding, RequestHandlerError> {
    let value = match selection {
        PhantomSelection::Policy { phantom } => return Ok(policy(*phantom)),
        PhantomSelection::Original => return Ok(AgentBinding::Constructor),
        PhantomSelection::Path { index } => {
            resolved_route.captured_path_parameters[usize::from(*index)].as_str()
        }
        PhantomSelection::Query { name, optional } => {
            match request.query_params().get(name).map(Vec::as_slice) {
                Some([value]) => value.as_str(),
                None if *optional => return Ok(AgentBinding::Constructor),
                None | Some([]) => {
                    return Err(RequestHandlerError::MissingValue {
                        expected: "agent ID query selector",
                    });
                }
                _ => {
                    return Err(RequestHandlerError::TooManyValues {
                        expected: "agent ID query selector",
                    });
                }
            }
        }
    };
    Uuid::parse_str(value)
        .map(|uuid| AgentBinding::Id(AgentId(uuid)))
        .map_err(|_| RequestHandlerError::ValueParsingFailed {
            value: value.into(),
            expected: "agent ID",
        })
}

pub(super) fn principal_from_request(
    request: &RichRequest,
) -> Result<Principal, RequestHandlerError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::model::agent::AgentMode;
    use golem_common::schema::{InputSchema, SchemaGraph, SchemaType};
    use golem_service_base::custom_api::{CompiledInputSchema, PathSegmentType};
    use test_r::test;

    #[test]
    async fn query_ownership_rejects_unknown_and_malformed_names_without_requiring_arguments() {
        use golem_service_base::custom_api::QueryOrHeaderType;
        let resolver = test_resolver(vec![test_route(1, "/", Some("POST"), "typed")]);
        let request = poem::Request::builder()
            .method(Method::POST)
            .uri("/".parse().unwrap())
            .header("host", "example.com")
            .finish();
        let route = resolver.resolve_matching_route(&request).await.unwrap();
        let super::super::RichRouteBehaviour::CallAgent(behaviour) = &route.route.behavior else {
            panic!("not typed")
        };
        let mut behaviour = behaviour.clone();
        behaviour.method_parameters = vec![
            MethodParameter::Query {
                query_parameter_name: "q".into(),
                parameter_type: QueryOrHeaderType::Primitive(PathSegmentType::Str),
            },
            MethodParameter::Query {
                query_parameter_name: "labels".into(),
                parameter_type: QueryOrHeaderType::Map {
                    key: PathSegmentType::Str,
                    value: PathSegmentType::Bool,
                },
            },
        ];
        behaviour.phantom_selection = PhantomSelection::Query {
            name: "instance".into(),
            optional: true,
        };
        for (query, valid) in [
            ("", true),
            ("q=x&labels%5Benv%5D=true&instance=x", true),
            ("v=3", false),
            ("labels=true", false),
            ("labels[]=true", false),
            ("labels[env][zone]=true", false),
            ("labels[env=true", false),
            ("q[x]=x", false),
            ("offset=0", false),
        ] {
            let request = RichRequest::new(
                poem::Request::builder()
                    .uri(format!("/?{query}").parse().unwrap())
                    .finish(),
            );
            assert_eq!(
                validate_query_parameters(&request, &behaviour, &[]).is_ok(),
                valid,
                "{query}"
            );
        }
        let request = RichRequest::new(
            poem::Request::builder()
                .uri("/?offset=0&live=sse&cursor=2".parse().unwrap())
                .finish(),
        );
        assert!(
            validate_query_parameters(&request, &behaviour, &["offset", "live", "cursor"]).is_ok()
        );
        for _ in 0..32 {
            for query in [
                "z=1&b=2&%61=3&q=x&instance=x&offset=0",
                "%61=3&offset=0&instance=x&q=x&b=2&z=1",
            ] {
                let request = RichRequest::new(
                    poem::Request::builder()
                        .uri(format!("/?{query}").parse().unwrap())
                        .finish(),
                );
                let error =
                    validate_query_parameters(&request, &behaviour, &["offset"]).unwrap_err();
                assert!(
                    matches!(error, RequestHandlerError::UnknownQueryParameter { ref name } if name == "a"),
                    "{error:?}"
                );
            }
        }
    }

    #[test]
    async fn bound_query_constraints_use_original_referenced_collection_schemas() {
        use golem_common::schema::schema_type::{NumericBound, NumericRestrictions};
        use golem_common::schema::{NamedField, SchemaTypeDef, TypeId};
        use golem_service_base::custom_api::QueryOrHeaderType;
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let handler = CallAgentHandler::new(harness.worker_service.clone());
        let resolver = test_resolver(vec![test_route(1, "/", Some("GET"), "typed")]);
        let request = poem::Request::builder()
            .uri("/".parse().unwrap())
            .header("host", "example.com")
            .finish();
        let route = resolver.resolve_matching_route(&request).await.unwrap();
        let super::super::RichRouteBehaviour::CallAgent(behaviour) = &route.route.behavior else {
            panic!("not typed")
        };
        let mut behaviour = behaviour.clone();
        let bounded = SchemaType::U32 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::Unsigned(2)),
                max: Some(NumericBound::Unsigned(8)),
                unit: None,
            }),
            metadata: Default::default(),
        };
        let reference = SchemaType::ref_to(TypeId::new("bounded"));
        for (schema, ty, good, bad) in [
            (
                reference.clone(),
                QueryOrHeaderType::Primitive(PathSegmentType::U32),
                "wire=3",
                "wire=1",
            ),
            (
                SchemaType::list(reference.clone()),
                QueryOrHeaderType::List {
                    name: None,
                    owner: None,
                    inner: Box::new(PathSegmentType::U32),
                },
                "wire=3&wire=7",
                "wire=3&wire=9",
            ),
            (
                SchemaType::map(reference.clone(), reference),
                QueryOrHeaderType::Map {
                    key: PathSegmentType::U32,
                    value: PathSegmentType::U32,
                },
                "wire[3]=7",
                "wire[1]=7",
            ),
        ] {
            behaviour.method_input = CompiledInputSchema {
                graph: SchemaGraph {
                    root: SchemaType::record(vec![]),
                    defs: vec![SchemaTypeDef {
                        id: TypeId::new("bounded"),
                        name: None,
                        body: bounded.clone(),
                    }],
                },
                input_schema: InputSchema::parameters([NamedField::user_supplied(
                    "amount", schema,
                )]),
            };
            behaviour.method_parameters = vec![MethodParameter::Query {
                query_parameter_name: "wire".into(),
                parameter_type: ty,
            }];
            for (query, valid) in [(good, true), (bad, false)] {
                let request = RichRequest::new(
                    poem::Request::builder()
                        .uri(format!("/?{query}").parse().unwrap())
                        .finish(),
                );
                let result = handler.resolve_method_arguments(
                    &route,
                    &request,
                    &behaviour,
                    ParsedRequestBody::Unused,
                );
                assert_eq!(result.is_ok(), valid, "{query}");
                if let Err(error) = result {
                    assert!(error.to_string().contains("amount"), "{error}");
                }
            }
        }
    }

    #[test]
    async fn invalid_query_fails_before_cache_metadata_or_body_reads() {
        use futures::stream;
        use golem_common::model::agent::CachePolicy;
        use golem_common::schema::NamedField;
        use golem_service_base::custom_api::{CompiledSchema, QueryOrHeaderType};
        let harness = crate::mcp::InvocationHarness::with_run_method(AgentMode::Durable);
        let handler = CallAgentHandler::new(harness.worker_service.clone());
        for method in ["GET", "HEAD", "DELETE", "POST"] {
            for query in ["v=3", "lbl=x", "lbl=x&lbl[env]=true"] {
                let resolver = test_resolver(vec![test_route(1, "/", Some(method), "typed")]);
                let request = poem::Request::builder()
                    .uri(format!("/?{query}").parse().unwrap())
                    .method(method.parse().unwrap())
                    .header("host", "example.com")
                    .header("content-type", "application/json")
                    .header("if-none-match", "*")
                    .body(poem::Body::from_bytes_stream(stream::poll_fn(
                        |_| -> std::task::Poll<Option<Result<bytes::Bytes, std::io::Error>>> {
                            panic!("query rejection must not read body")
                        },
                    )));
                let route = resolver.resolve_matching_route(&request).await.unwrap();
                let super::super::RichRouteBehaviour::CallAgent(behaviour) = &route.route.behavior
                else {
                    panic!("not typed")
                };
                let mut behaviour = behaviour.clone();
                behaviour.method_input = CompiledInputSchema {
                    graph: SchemaGraph::empty(),
                    input_schema: InputSchema::parameters([NamedField::user_supplied(
                        "labels",
                        SchemaType::map(SchemaType::string(), SchemaType::bool()),
                    )]),
                };
                behaviour.method_parameters = vec![MethodParameter::Query {
                    query_parameter_name: "lbl".into(),
                    parameter_type: QueryOrHeaderType::Map {
                        key: PathSegmentType::Str,
                        value: PathSegmentType::Bool,
                    },
                }];
                if method == "POST" {
                    behaviour.method_input.input_schema = InputSchema::parameters([
                        NamedField::user_supplied(
                            "labels",
                            SchemaType::map(SchemaType::string(), SchemaType::bool()),
                        ),
                        NamedField::user_supplied("payload", SchemaType::string()),
                    ]);
                    behaviour
                        .method_parameters
                        .push(MethodParameter::JsonObjectBodyField {
                            field_index: 0.into(),
                        });
                    behaviour.body = RequestBodySchema::JsonBody {
                        expected: CompiledSchema {
                            graph: SchemaGraph::anonymous(SchemaType::record_from_fields([(
                                "payload",
                                SchemaType::string(),
                            )])),
                        },
                    };
                }
                behaviour.read_only = Some(ReadOnlyConfig {
                    cache_policy: CachePolicy::UntilWrite(Default::default()),
                    uses_principal: false,
                });
                let error = handler
                    .handle_call_agent_behaviour(&mut RichRequest::new(request), &route, &behaviour)
                    .await
                    .unwrap_err();
                if query == "v=3" {
                    assert!(
                        matches!(error, RequestHandlerError::UnknownQueryParameter { ref name } if name == "v")
                    );
                } else {
                    assert!(
                        matches!(error, RequestHandlerError::InvalidParameter { ref name, ref error } if name == "labels" && error.contains("maps require name[key]")),
                        "{error:?}"
                    );
                }
                assert!(harness.metadata_reads.lock().unwrap().is_empty());
                assert!(harness.contexts.lock().unwrap().is_empty());
            }
        }
    }

    #[test]
    async fn explicit_agent_selection_validates_decoded_queries_and_capture_ordinals() {
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
            route_agent_binding(
                &RichRequest::new(route_request),
                &resolved,
                &path,
                |_| panic!("explicit selection must not use policy")
            )
            .unwrap(),
            AgentBinding::Id(AgentId(p))
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
                    route_agent_binding(&request, &resolved, &selection, |_| panic!(
                        "explicit selection must not use policy"
                    ))
                    .unwrap(),
                    AgentBinding::Id(AgentId(p))
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
                    route_agent_binding(&request, &resolved, &selection, |_| panic!(
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
            let result = route_agent_binding(&request, &resolved, &selection, |_| {
                panic!("optional omission must not use policy")
            });
            if optional {
                assert_eq!(result.unwrap(), AgentBinding::Constructor);
            } else {
                assert!(result.is_err());
            }
        }
        let request = RichRequest::new(poem::Request::builder().finish());
        assert_eq!(
            route_agent_binding(
                &request,
                &resolved,
                &PhantomSelection::Original,
                |_| panic!("original must not use policy")
            )
            .unwrap(),
            AgentBinding::Constructor
        );
        for phantom in [false, true] {
            assert_eq!(
                route_agent_binding(
                    &request,
                    &resolved,
                    &PhantomSelection::Policy { phantom },
                    |flag| {
                        assert_eq!(flag, phantom);
                        AgentBinding::Fresh
                    }
                )
                .unwrap(),
                AgentBinding::Fresh
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
        let agent_type = golem_common::model::agent::AgentTypeName("selected".into());
        let constructor_key =
            CallAgentHandler::route_constructor(&resolved, &agent_type, &constructor, &bindings)
                .unwrap();
        assert_eq!(constructor_key.to_string(), "selected(\"d7\",\"west\")");
        resolved.captured_path_parameters[0] = "d9".into();
        let other =
            CallAgentHandler::route_constructor(&resolved, &agent_type, &constructor, &bindings)
                .unwrap();
        assert_eq!(other.to_string(), "selected(\"d9\",\"west\")");
    }
}
