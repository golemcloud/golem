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

//! Session lifecycle: creating the invocation session behind a stream URL,
//! PUT/DELETE handling and the session summary, plus the input/output schema
//! inspection that decides which slots a route declares.

use super::super::call_agent::principal_from_request;
use super::super::error::RequestHandlerError;
use super::super::route_resolver::ResolvedRouteEntry;
use super::super::{ResponseBody, RichRequest, RouteExecutionResult};
use super::encoding::{metadata_response, offset_text};
use super::expiry::{add_expiry_headers, policies_match};
use super::{DurableStreamsHandler, body_response, response, route_method};
use golem_api_grpc::proto::golem::worker::{
    AgentInvocationMode, InvocationContext, InvocationStart,
};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    CreateStreamSessionRequest, CreateStreamSessionSuccess, DurableStreamAttachmentControlRequest,
    ExportStreamControl, ExportStreamControlResult, StreamSessionCreationIntent,
    StreamSessionExpiryPolicy, StreamSlotReadAdmission,
};
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::{
    FieldSource, SchemaType, SchemaValue, schema_value_to_proto_with_streams,
};
use golem_service_base::custom_api::{
    CallAgentBehaviour, DurableStreamRepresentation, DurableStreamSlot, MethodParameter,
};
use golem_service_base::model::auth::AuthCtx;
use http::{HeaderName, Method, StatusCode};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

impl DurableStreamsHandler {
    /// Starts (or idempotently re-attaches to) the invocation session identified
    /// by `session`. Stream-typed input fields are bound to host endpoints in
    /// declaration order; the remaining arguments come from the request.
    pub(super) async fn create(
        &self,
        request: &mut RichRequest,
        route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
        agent_id: &AgentId,
        session: &str,
        creation_intent: StreamSessionCreationIntent,
        expiry_policy: Option<StreamSessionExpiryPolicy>,
    ) -> Result<CreateStreamSessionSuccess, RequestHandlerError> {
        let body = request.parse_request_body(&route.route.body).await?;
        let args = self
            .call_agent
            .resolve_method_arguments(route, request, behaviour, body)?;
        let mut args = args.into_iter();
        let mut transport_id = 0u64;
        let fields = behaviour
            .method_input
            .input_schema
            .fields()
            .iter()
            .filter(|field| matches!(field.source, FieldSource::UserSupplied))
            .map(|field| {
                if matches!(
                    behaviour
                        .method_input
                        .graph
                        .resolve_ref(&field.schema)
                        .map_err(|e| anyhow::anyhow!(e.to_string()))?,
                    SchemaType::Stream { .. }
                ) {
                    let value =
                        SchemaValue::Stream(SchemaValueStream::from_host_endpoint(transport_id));
                    transport_id += 1;
                    Ok(value)
                } else {
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("missing non-stream method argument"))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        if args.next().is_some() {
            return Err(anyhow::anyhow!("too many non-stream method arguments").into());
        }
        let input = schema_value_to_proto_with_streams(SchemaValue::Record { fields }, |stream| {
            stream.take_host_endpoint::<u64>()
        })
        .map_err(|e| anyhow::anyhow!("invalid durable stream input: {e}"))?;
        let principal = principal_from_request(request)?;
        let start = InvocationStart {
            agent_id: Some(agent_id.clone().into()),
            method_name: Some(behaviour.method_name.clone()),
            external_tool: None,
            input: Some(input),
            idempotency_key: Some(IdempotencyKey::new(session.to_owned()).into()),
            context: Some(InvocationContext {
                parent: None,
                env: Default::default(),
                tracing: Some(request.invocation_context().into()),
            }),
            auth_ctx: Some(AuthCtx::System.into()),
            principal: Some(principal.into()),
            environment_id: Some(route.route.environment_id.into()),
            config: vec![],
            component_owner_account_id: Some(route.route.account_id.into()),
            mode: AgentInvocationMode::Await as i32,
            schedule_at: None,
            freshness_disposition: 0,
            attempt_id: Some(Uuid::new_v4().into()),
            expected_callee_fingerprint: None,
            durable_input_mappings: vec![],
            scope_card: None,
            origin_invocation: None,
        };
        self.worker_service
            .create_stream_session(
                agent_id,
                CreateStreamSessionRequest {
                    public_session_id: session.to_owned(),
                    expiry_policy,
                    creation_intent: creation_intent as i32,
                    invocation: Some(start),
                },
            )
            .await
            .map_err(Into::into)
    }

    /// `PUT` on the session collection: creates a session under a freshly
    /// generated id and points the client at it.
    pub(super) async fn create_generated(
        &self,
        request: &mut RichRequest,
        route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
        agent_id: &AgentId,
        session: &str,
        expiry_policy: Option<StreamSessionExpiryPolicy>,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let created = self
            .create(
                request,
                route,
                behaviour,
                agent_id,
                session,
                StreamSessionCreationIntent::ExplicitPut,
                expiry_policy,
            )
            .await?;
        let mut result = response(created_status(created.replayed));
        result.headers.insert(
            http::header::LOCATION,
            format!(
                "{}/invocations/{session}",
                request.underlying.uri().path().trim_end_matches('/')
            ),
        );
        Ok(result)
    }

    /// `PUT` on an explicit session, optionally scoped to one slot. A slot PUT
    /// must carry no body, must match the slot's declared content type and only
    /// creates the session when all method arguments come from the URL.
    pub(super) async fn put(
        &self,
        request: &mut RichRequest,
        route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
        agent_id: &AgentId,
        session: &str,
        slot: Option<&str>,
        expiry_policy: Option<StreamSessionExpiryPolicy>,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        if let Some(slot) = slot {
            let Some(declared_slot) = declared_slot(behaviour, slot)? else {
                return Ok(response(StatusCode::NOT_FOUND));
            };
            let mut body = request.underlying.take_body().into_async_read();
            if body
                .read(&mut [0u8; 1])
                .await
                .map_err(anyhow::Error::from)?
                != 0
            {
                return Ok(response(StatusCode::BAD_REQUEST));
            }
            let args_from_url = arguments_come_from_url(behaviour);
            if let Some(metadata) = self
                .read_slot(route, agent_id, session, slot, Vec::new(), 0, 0)
                .await?
            {
                if !policies_match(&metadata.expiry_policy, &expiry_policy) {
                    return Ok(response(StatusCode::CONFLICT));
                }
                if metadata.tombstoned
                    || content_type_mismatch(request, &declared_slot.content_type)?
                {
                    return Ok(response(StatusCode::CONFLICT));
                }
                if args_from_url {
                    let created = self
                        .create(
                            request,
                            route,
                            behaviour,
                            agent_id,
                            session,
                            StreamSessionCreationIntent::ExplicitPut,
                            expiry_policy,
                        )
                        .await?;
                    let Some(metadata) = self
                        .read_slot_admitted(
                            route,
                            agent_id,
                            session,
                            slot,
                            Vec::new(),
                            0,
                            0,
                            StreamSlotReadAdmission::Continuation,
                            created.invocation_key.clone(),
                        )
                        .await?
                    else {
                        return Err(anyhow::anyhow!(
                            "created stream session has no requested slot"
                        )
                        .into());
                    };
                    return created_slot_response(&metadata, created.replayed, declared_slot);
                }
                return metadata_response(&metadata, true, declared_slot);
            }
            if self
                .read_slot(route, agent_id, session, "", Vec::new(), 0, 0)
                .await?
                .is_some()
            {
                return Ok(response(StatusCode::NOT_FOUND));
            }
            if content_type_mismatch(request, &declared_slot.content_type)? {
                return Ok(response(StatusCode::CONFLICT));
            }
            if !args_from_url {
                return Ok(response(StatusCode::BAD_REQUEST));
            }
        }
        let created = self
            .create(
                request,
                route,
                behaviour,
                agent_id,
                session,
                StreamSessionCreationIntent::ExplicitPut,
                expiry_policy,
            )
            .await?;
        match slot {
            Some(slot) => match self
                .read_slot_admitted(
                    route,
                    agent_id,
                    session,
                    slot,
                    Vec::new(),
                    0,
                    0,
                    StreamSlotReadAdmission::Continuation,
                    created.invocation_key.clone(),
                )
                .await?
            {
                Some(metadata) => {
                    let declared_slot = declared_slot(behaviour, slot)?.ok_or_else(|| {
                        anyhow::anyhow!("created undeclared Durable Streams slot '{slot}'")
                    })?;
                    created_slot_response(&metadata, created.replayed, declared_slot)
                }
                None => Err(anyhow::anyhow!("created stream session has no requested slot").into()),
            },
            None => {
                let mut result = response(StatusCode::OK);
                result.status = created_status(created.replayed);
                Ok(result)
            }
        }
    }

    /// `DELETE` on a session or one of its slots, forwarded to the executor as
    /// an export stream control request.
    pub(super) async fn delete(
        &self,
        route: &ResolvedRouteEntry,
        agent_id: &AgentId,
        session: String,
        slot: Option<String>,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let result = self
            .worker_service
            .control_export_stream(
                agent_id,
                DurableStreamAttachmentControlRequest {
                    producer_agent_id: Some(agent_id.clone().into()),
                    producer_environment_id: Some(route.route.environment_id.into()),
                    auth_ctx: Some(AuthCtx::System.into()),
                    export_control: Some(ExportStreamControl {
                        session,
                        slot,
                        expected_method: route_method(route).to_owned(),
                    }),
                    ..Default::default()
                },
            )
            .await?;
        Ok(response(match result {
            ExportStreamControlResult::Applied => StatusCode::NO_CONTENT,
            ExportStreamControlResult::NotFound => StatusCode::NOT_FOUND,
            ExportStreamControlResult::Gone => StatusCode::GONE,
            ExportStreamControlResult::Unspecified => {
                return Err(anyhow::anyhow!("unspecified export stream control result").into());
            }
        }))
    }

    /// `GET`/`HEAD` on a session: a JSON summary of every slot and whether the
    /// whole session has finished.
    pub(super) async fn describe(
        &self,
        request: &RichRequest,
        route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
        agent_id: &AgentId,
        session: &str,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        let read = self
            .read_slot_admitted(
                route,
                agent_id,
                session,
                "",
                Vec::new(),
                0,
                0,
                if request.underlying.method() == Method::HEAD {
                    StreamSlotReadAdmission::Head
                } else {
                    StreamSlotReadAdmission::TouchingOriginGet
                },
                None,
            )
            .await?;
        let Some(read) = read else {
            return Ok(response(StatusCode::NOT_FOUND));
        };
        let mut streams = Vec::new();
        let mut closed = true;
        for slot in &read.slots {
            let declared_slot = behaviour
                .durable_streams
                .as_ref()
                .and_then(|policy| policy.slot_by_canonical_name(slot))
                .ok_or_else(|| {
                    anyhow::anyhow!("executor returned undeclared Durable Streams slot '{slot}'")
                })?
                .clone();
            let Some(metadata) = self
                .read_slot_admitted(
                    route,
                    agent_id,
                    session,
                    slot,
                    Vec::new(),
                    0,
                    0,
                    StreamSlotReadAdmission::Continuation,
                    read.invocation_key.clone(),
                )
                .await?
            else {
                return Ok(response(StatusCode::NOT_FOUND));
            };
            closed &= metadata.closed || metadata.tombstoned;
            streams.push(serde_json::json!({
                "name": declared_slot.public_name,
                "contentType": declared_slot.content_type,
                "nextOffset": offset_text(&metadata.head_offset)?,
                "closed": metadata.closed,
                "cancelled": metadata.cancelled,
                "deleted": metadata.tombstoned,
            }));
        }
        let fork = read
            .fork
            .as_ref()
            .map(|fork| {
                Ok::<_, RequestHandlerError>(serde_json::json!({
                    "sourcePath": fork.source_path,
                    "forkOffset": offset_text(&fork.fork_offset)?,
                    "subOffset": fork.sub_offset,
                }))
            })
            .transpose()?;
        let body = serde_json::to_vec(
            &serde_json::json!({"session": session, "streams": streams, "closed": closed, "fork": fork}),
        )
        .map_err(anyhow::Error::from)?;
        let mut result = body_response(StatusCode::OK, body, "application/json");
        result
            .headers
            .insert(http::header::CACHE_CONTROL, "no-store".into());
        result
            .headers
            .insert(http::header::CONTENT_TYPE, "application/json".into());
        result
            .headers
            .insert(HeaderName::from_static("stream-closed"), closed.to_string());
        if request.underlying.method() == Method::HEAD {
            add_expiry_headers(&mut result, &read.expiry_policy);
            result.body = ResponseBody::NoBody;
        }
        Ok(result)
    }
}

fn created_status(replayed: bool) -> StatusCode {
    if replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    }
}

fn created_slot_response(
    metadata: &golem_api_grpc::proto::golem::workerexecutor::v1::ReadStreamSlotSuccess,
    replayed: bool,
    slot: &DurableStreamSlot,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    if metadata.tombstoned {
        return Ok(response(StatusCode::CONFLICT));
    }
    let mut result = metadata_response(metadata, true, slot)?;
    result.status = created_status(replayed);
    Ok(result)
}

/// True when every non-stream method argument is bound to a path or query
/// parameter, so a body-less request can still create the session.
pub(super) fn arguments_come_from_url(behaviour: &CallAgentBehaviour) -> bool {
    behaviour.method_parameters.iter().all(|param| {
        matches!(
            param,
            MethodParameter::Path { .. } | MethodParameter::Query { .. }
        )
    })
}

pub(super) fn content_type_mismatch(
    request: &RichRequest,
    expected: &str,
) -> Result<bool, RequestHandlerError> {
    Ok(request
        .header_string_value("content-type")?
        .is_some_and(|actual| {
            !actual
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case(expected)
        }))
}

pub(super) fn declared_slot<'a>(
    behaviour: &'a CallAgentBehaviour,
    slot: &str,
) -> Result<Option<&'a DurableStreamSlot>, RequestHandlerError> {
    Ok(behaviour
        .durable_streams
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Durable Streams route has no resolved policy"))?
        .slot_by_canonical_name(slot))
}

pub(super) fn canonical_content_type(representation: DurableStreamRepresentation) -> &'static str {
    match representation {
        DurableStreamRepresentation::Json => "application/json",
        DurableStreamRepresentation::Bytes => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::golem::workerexecutor::v1::ReadStreamSlotSuccess;
    use test_r::test;

    #[test]
    fn public_mime_comparison_uses_case_insensitive_essence() {
        let request = |content_type: Option<&str>| {
            let mut builder = poem::Request::builder();
            if let Some(content_type) = content_type {
                builder = builder.header(http::header::CONTENT_TYPE, content_type);
            }
            RichRequest::new(builder.finish())
        };

        assert!(
            !content_type_mismatch(
                &request(Some("Application/Vnd.Golem.Fragment; charset=binary")),
                "application/vnd.golem.fragment",
            )
            .unwrap()
        );
        assert!(
            content_type_mismatch(
                &request(Some("application/octet-stream")),
                "application/vnd.golem.fragment",
            )
            .unwrap()
        );
        assert!(!content_type_mismatch(&request(None), "application/vnd.golem.fragment").unwrap());
    }

    #[test]
    fn compiled_representation_maps_to_executor_content_type() {
        assert_eq!(
            canonical_content_type(DurableStreamRepresentation::Json),
            "application/json"
        );
        assert_eq!(
            canonical_content_type(DurableStreamRepresentation::Bytes),
            "application/octet-stream"
        );
    }

    #[test]
    fn tombstoned_created_slot_is_a_conflict_not_a_success() {
        let metadata = ReadStreamSlotSuccess {
            tombstoned: true,
            ..Default::default()
        };
        let slot = DurableStreamSlot {
            canonical_name: "$result".into(),
            public_name: "responses".into(),
            direction: golem_service_base::custom_api::DurableStreamSlotDirection::Output,
            content_type: "application/json".into(),
            representation: DurableStreamRepresentation::Json,
        };
        for replayed in [false, true] {
            let response = created_slot_response(&metadata, replayed, &slot).unwrap();
            assert_eq!(response.status, StatusCode::CONFLICT);
            assert_eq!(response.headers[&http::header::CACHE_CONTROL], "no-store");
        }
    }
}
