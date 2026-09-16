// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// See http://license.golem.cloud/LICENSE

use crate::service::worker::{PublicToolSessionStart, StartedPublicAgentSession, WorkerService};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures::StreamExt;
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::worker::{
    InputStreamEnd, InputStreamItem, InvocationAccepted, InvocationRequest, StreamCancel,
    StreamCancelReason, StreamCancelRole, input_stream_item, invocation_request,
    invocation_response, invocation_session_completion, invocation_session_result,
};
use golem_common::model::IdempotencyKey;
use golem_common::model::agent::{OwnerKind, Principal};
use golem_common::model::card::{
    AgentResourcePattern, AgentVerb, ClassPermissionTarget, EffectiveSurface, GrantSurface,
    PermissionTarget,
    owner::{AgentOwnerLeafPattern, AgentOwnerPattern},
};
use golem_common::model::invocation_session_public::{PublicNativeToolTarget, PublicTypedValue};
use golem_common::model::oplog::PublicExternalToolResult;
use golem_common::model::tool::{SerializableToolError, SerializableToolRpcError};
use golem_common::schema::public_json::{PublicSchemaValueError, encode_public_schema_value};
use golem_common::schema::validation::{is_equivalent_cross_graph, validate_value};
use golem_common::schema::{FALLBACK_OUTPUT_FIELD_NAME, SchemaType, SchemaValue, TypedSchemaValue};
use golem_schema::schema::render::to_json_value_redacted;
use golem_service_base::mcp::{CompiledMcp, CompiledMcpToolExport};
use golem_service_base::model::auth::AuthCtx;
use rmcp::{
    ErrorData,
    model::{CallToolResult, Content, Tool, ToolAnnotations},
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

const MAX_OUTPUT: usize = 16 * 1024 * 1024;
const INPUT_CHUNK: usize = 64 * 1024;

pub fn tool_metadata(export: &CompiledMcpToolExport) -> Result<Tool, ErrorData> {
    let (_, body) = export.command().map_err(invalid)?;
    Ok(Tool {
        name: export.mcp_name.clone().into(),
        title: None,
        description: Some(export.description.clone().into()),
        input_schema: Arc::new(export.input_json_schema().map_err(invalid)?),
        output_schema: export.output_json_schema().map_err(invalid)?.map(Arc::new),
        annotations: body.annotations.map(|hints| ToolAnnotations {
            title: None,
            read_only_hint: Some(hints.read_only),
            destructive_hint: Some(hints.destructive),
            idempotent_hint: Some(hints.idempotent),
            open_world_hint: Some(hints.open_world),
        }),
        execution: None,
        icons: None,
        meta: None,
    })
}

fn invocation_auth(
    deployment: &CompiledMcp,
    export: &CompiledMcpToolExport,
    key: &IdempotencyKey,
) -> AuthCtx {
    let target = PermissionTarget::Agent(ClassPermissionTarget {
        owner: AgentOwnerPattern::Agent {
            account: deployment.account_email.clone(),
            application: deployment.application_name.clone(),
            environment: deployment.environment_name.clone(),
            component: export.owner_component_name.clone(),
            agent: AgentOwnerLeafPattern::Agent(OwnerKind::external_tool_instance_name(key)),
        },
        verb: Some(AgentVerb::Invoke),
        resource: AgentResourcePattern::Any,
    });
    AuthCtx::agent_with_effective_surface(
        deployment.account_id,
        deployment.account_email.clone(),
        EffectiveSurface {
            source_card_ids: vec![],
            lower: vec![GrantSurface {
                positive: vec![target],
                negative: vec![],
            }],
            upper: vec![],
        },
    )
}

pub async fn invoke(
    worker_service: &Arc<WorkerService>,
    deployment: &CompiledMcp,
    export: &CompiledMcpToolExport,
    input: TypedSchemaValue,
    stdin: Option<Vec<u8>>,
    cancellation: CancellationToken,
) -> Result<CallToolResult, ErrorData> {
    let (_, body) = export.command().map_err(invalid)?;
    let key = IdempotencyKey::fresh();
    let auth = invocation_auth(deployment, export, &key);
    let input = PublicTypedValue {
        schema: input.graph().clone(),
        value: encode_public_schema_value(
            input.graph(),
            &input.graph().root,
            input.value(),
            reject_capability,
        )
        .map_err(|e| invalid(e.to_string()))?,
    };
    let (sender, receiver) = mpsc::channel(2);
    let session = worker_service
        .invoke_internal_tool_session_v1(
            PublicToolSessionStart {
                application: deployment.application_name.0.clone(),
                environment: deployment.environment_name.0.clone(),
                target: PublicNativeToolTarget::Component {
                    component_id: export.owner_component_id.0,
                },
                tool_name: export.tool_name.to_string(),
                command_path: export.command_path.clone(),
                input,
                stdin: stdin.is_some(),
                stdout: body.stdout.is_some(),
                idempotency_key: key.value,
                attempt_id: uuid::Uuid::new_v4(),
                expected_deployment_revision: Some(deployment.deployment_revision),
            },
            Box::pin(ReceiverStream::new(receiver)),
            auth,
            Principal::anonymous(),
            |_| Ok(()),
        )
        .await
        .map_err(|e| invalid(format!("native tool admission failed: {e:?}")))?;
    let (result, stdout) =
        collect_session(session, sender, stdin, body.stdout.is_some(), cancellation).await?;
    project_result(export, result, stdout)
}

async fn collect_session(
    mut session: StartedPublicAgentSession,
    sender: mpsc::Sender<InvocationRequest>,
    stdin: Option<Vec<u8>>,
    stdout_requested: bool,
    cancellation: CancellationToken,
) -> Result<(PublicExternalToolResult, Vec<u8>), ErrorData> {
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&session.initial_request)
        .map_err(invalid)?;
    let mut accepted: Option<InvocationAccepted> = None;
    let mut input_position = 0;
    let mut input_done = stdin.is_none();
    let mut stdout_done = !stdout_requested;
    let mut stdout = Vec::new();
    let mut stdout_error = None;
    let mut result = None;
    let outcome = async {
        loop {
            let next_input = if !input_done { accepted.as_ref().and_then(|a| {
                let stream = a.tool_stdin_stream_id?;
                let mapping = a.stream_mappings.iter().find(|m| m.transport_stream_id == stream)?;
                let durable_stream_id = mapping.handle.as_ref()?.stream_id;
                let bytes = stdin.as_ref()?;
                let request = if input_position == bytes.len() {
                    invocation_request::Request::InputEnd(InputStreamEnd {
                        transport_stream_id: stream, sequence: input_position as u64,
                        durable_stream_id, epoch: a.epoch,
                    })
                } else {
                    invocation_request::Request::InputItem(InputStreamItem {
                        transport_stream_id: stream, sequence: input_position as u64,
                        payload: Some(input_stream_item::Payload::PackedU8(bytes[input_position..bytes.len().min(input_position + INPUT_CHUNK)].to_vec())),
                        durable_stream_id, epoch: a.epoch,
                    })
                };
                Some(InvocationRequest { request: Some(request) })
            }) } else { None };
            tokio::select! {
                _ = cancellation.cancelled() => return Err(invalid("tool invocation cancelled")),
                permit = sender.reserve(), if next_input.is_some() => {
                    let permit = permit.map_err(|_| invalid("native input channel closed"))?;
                    let frame = next_input.unwrap();
                    state.validate_trusted_request(&frame).map_err(invalid)?;
                    match &frame.request {
                        Some(invocation_request::Request::InputItem(InputStreamItem { payload: Some(input_stream_item::Payload::PackedU8(bytes)), .. })) => input_position += bytes.len(),
                        _ => input_done = true,
                    }
                    permit.send(frame);
                }
                response = session.responses.next() => {
                    let response = response.ok_or_else(|| invalid("native session ended before completion"))?
                        .map_err(|e| invalid(format!("native session transport failed: {e}")))?;
                    state.validate_response(&response).map_err(invalid)?;
                    match response.response {
                        Some(invocation_response::Response::Accepted(a)) => {
                            if a.tool_stdin_stream_id.is_some() != stdin.is_some() || a.tool_stdout_stream_id.is_some() != stdout_requested {
                                return Err(invalid("native byte stream mappings differ from the request"));
                            }
                            accepted = Some(a);
                        }
                        Some(invocation_response::Response::OutputItem(item)) => {
                            let bytes = if !item.packed_u8.is_empty() { item.packed_u8 } else {
                                match item.value.map(SchemaValue::try_from).transpose().map_err(invalid)? {
                                    Some(SchemaValue::U8(byte)) => vec![byte],
                                    _ => return Err(invalid("native stdout contains a non-byte value")),
                                }
                            };
                            if stdout.len() + bytes.len() > MAX_OUTPUT { return Err(invalid("native stdout exceeds the 16 MiB MCP limit")); }
                            stdout.extend(bytes);
                        }
                        Some(invocation_response::Response::OutputEnd(_)) => stdout_done = true,
                        Some(invocation_response::Response::OutputError(error)) => { stdout_done = true; stdout_error = Some(error.details); }
                        Some(invocation_response::Response::StreamCancel(cancel)) => {
                            if accepted.as_ref().and_then(|a| a.tool_stdin_stream_id) == Some(cancel.transport_stream_id) { input_done = true; }
                            else { stdout_done = true; stdout_error = Some("stdout cancelled".to_string()); }
                        }
                        Some(invocation_response::Response::InputAck(_)) => {},
                        Some(invocation_response::Response::Result(value)) => {
                            let Some(invocation_session_result::Result::ToolResult(value)) = value.result else { return Err(invalid("native tool returned an agent-method result")); };
                            result = Some(PublicExternalToolResult::try_from(value).map_err(invalid)?);
                        }
                        Some(invocation_response::Response::Finished(completion)) => {
                            if let Some(invocation_session_completion::Outcome::Failure(failure)) = completion.outcome {
                                return Ok((PublicExternalToolResult::Failure(SerializableToolRpcError::RemoteInternalError(failure.message)), stdout));
                            }
                            if !stdout_done { return Err(invalid("native session completed before stdout")); }
                            let result = result.ok_or_else(|| invalid("native tool completed without a result"))?;
                            if matches!(result, PublicExternalToolResult::Success(_)) && let Some(error) = stdout_error {
                                return Ok((PublicExternalToolResult::Failure(SerializableToolRpcError::RemoteInternalError(error)), stdout));
                            }
                            return Ok((result, stdout));
                        }
                        Some(invocation_response::Response::Rejected(rejected)) => return Err(invalid(rejected.error)),
                        _ => return Err(invalid("native tool attachment was revoked or returned an invalid frame")),
                    }
                }
            }
        }
    }.await;
    if outcome.is_err()
        && let Some(accepted) = accepted
    {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            for (stream, role) in [
                (
                    accepted.tool_stdin_stream_id.filter(|_| !input_done),
                    StreamCancelRole::InputProducer,
                ),
                (
                    accepted.tool_stdout_stream_id.filter(|_| !stdout_done),
                    StreamCancelRole::OutputConsumer,
                ),
            ] {
                if let Some(stream) = stream {
                    let durable_stream_id = accepted
                        .stream_mappings
                        .iter()
                        .find(|m| m.transport_stream_id == stream)
                        .and_then(|m| m.handle.as_ref())
                        .and_then(|h| h.stream_id);
                    let _ = sender
                        .send(InvocationRequest {
                            request: Some(invocation_request::Request::StreamCancel(
                                StreamCancel {
                                    transport_stream_id: stream,
                                    producer_sequence: if role == StreamCancelRole::InputProducer {
                                        input_position as u64
                                    } else {
                                        0
                                    },
                                    role: role as i32,
                                    reason: StreamCancelReason::Cancelled as i32,
                                    details: None,
                                    durable_stream_id,
                                    epoch: accepted.epoch,
                                    durable_offset: vec![],
                                },
                            )),
                        })
                        .await;
                }
            }
        })
        .await;
    }
    outcome
}

fn project_result(
    export: &CompiledMcpToolExport,
    result: PublicExternalToolResult,
    stdout: Vec<u8>,
) -> Result<CallToolResult, ErrorData> {
    let (_, body) = export.command().map_err(invalid)?;
    let result = match result {
        PublicExternalToolResult::Success(result) => result.result,
        PublicExternalToolResult::Failure(SerializableToolRpcError::RemoteToolError(error)) => {
            return match *error {
                SerializableToolError::CustomError(value) => {
                    let payload = to_json_value_redacted(
                        value.payload.graph(),
                        &value.payload.graph().root,
                        value.payload.value(),
                    )
                    .map_err(|e| invalid(e.to_string()))?;
                    Ok(CallToolResult::error(vec![Content::text(
                        serde_json::json!({"name": value.name, "payload": payload}).to_string(),
                    )]))
                }
                error => Err(invalid(format!("{error:?}"))),
            };
        }
        PublicExternalToolResult::Failure(error) => {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "{error:?}"
            ))]));
        }
    };
    let structured_content = match (&body.result, result) {
        (None, None) => None,
        (Some(spec), Some(value)) => {
            if !is_equivalent_cross_graph(
                value.graph(),
                &value.graph().root,
                &export.definition.schema,
                &spec.type_,
            ) {
                return Err(invalid("native result schema differs from exported schema"));
            }
            validate_value(value.graph(), &value.graph().root, value.value())
                .map_err(|e| invalid(format!("invalid native result: {e:?}")))?;
            let json = to_json_value_redacted(value.graph(), &value.graph().root, value.value())
                .map_err(|e| invalid(e.to_string()))?;
            Some(if matches!(&spec.type_, SchemaType::Record { .. }) {
                json
            } else {
                json!({FALLBACK_OUTPUT_FIELD_NAME: json})
            })
        }
        _ => return Err(invalid("native result does not match the exported command")),
    };
    let mut content = Vec::new();
    if let Some(value) = &structured_content {
        content.push(Content::text(value.to_string()));
    }
    if let Some(spec) = &body.stdout {
        let mime = spec
            .mime
            .first()
            .map(String::as_str)
            .unwrap_or("application/octet-stream");
        let projected = if spec.mime.iter().all(|mime| mime.starts_with("text/")) {
            json!({"type":"text", "text": String::from_utf8(stdout).map_err(|_| invalid("stdout is not valid UTF-8"))?})
        } else if mime.starts_with("image/") || mime.starts_with("audio/") {
            json!({"type":if mime.starts_with("image/") {"image"} else {"audio"}, "data":STANDARD.encode(stdout), "mimeType":mime})
        } else {
            json!({"type":"resource", "resource":{"uri":format!("golem-tool://{}/stdout", export.mcp_name),"mimeType":mime,"blob":STANDARD.encode(stdout)}})
        };
        content.push(serde_json::from_value(projected).map_err(|e| invalid(e.to_string()))?);
    }
    Ok(CallToolResult {
        content,
        structured_content,
        is_error: Some(false),
        meta: None,
    })
}

fn invalid(message: impl Into<String>) -> ErrorData {
    ErrorData::invalid_params(message.into(), None)
}

fn reject_capability(
    _: &golem_schema::schema::SchemaValueStream,
    _: Option<&SchemaType>,
) -> Result<golem_common::schema::public_json::PublicStreamReference, PublicSchemaValueError> {
    Err(PublicSchemaValueError::new(
        golem_common::model::invocation_session_public::PublicErrorCode::UnsupportedValue,
        "capabilities are not supported by native MCP tools",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invocation_session_token::SessionInvocationTarget;
    use futures::stream;
    use golem_api_grpc::proto::golem::schema::{
        SchemaValue as ProtoSchemaValue, TypedSchemaValue as ProtoTypedSchemaValue, schema_value,
    };
    use golem_api_grpc::proto::golem::worker::{
        ExternalToolInvocation, InvocationStart, invocation_request,
    };
    use golem_common::model::AgentId;
    use golem_common::model::application::ApplicationName;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::environment::EnvironmentName;
    use golem_common::schema::SchemaGraph;
    use test_r::{test, timeout};

    fn empty_session() -> StartedPublicAgentSession {
        StartedPublicAgentSession {
            responses: Box::pin(stream::pending()),
            initial_request: InvocationRequest {
                request: Some(invocation_request::Request::Start(InvocationStart {
                    idempotency_key: Some(golem_api_grpc::proto::golem::worker::IdempotencyKey {
                        value: "native-mcp-test".to_string(),
                    }),
                    external_tool: Some(ExternalToolInvocation {
                        tool_name: "tool".to_string(),
                        input: Some(ProtoTypedSchemaValue {
                            graph: Some(
                                SchemaGraph {
                                    defs: vec![],
                                    root: SchemaType::u8(),
                                }
                                .into(),
                            ),
                            value: Some(ProtoSchemaValue {
                                value: Some(schema_value::Value::U8Value(1)),
                            }),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                })),
            },
            schema: SchemaGraph::empty(),
            output_schema: None,
            agent_id: AgentId {
                component_id: ComponentId(uuid::Uuid::from_u128(20)),
                agent_id: "native-mcp-test".to_string(),
            },
            application: ApplicationName("application".to_string()),
            environment: EnvironmentName("environment".to_string()),
            target: SessionInvocationTarget::ExternalTool {
                tool_name: "tool".to_string(),
                command_path: vec![],
            },
            component_revision: ComponentRevision::new(1).unwrap(),
        }
    }

    #[test]
    #[timeout("5s")]
    async fn cancellation_interrupts_a_session_waiting_for_output() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let (sender, _receiver) = mpsc::channel(2);

        let error = collect_session(empty_session(), sender, None, false, cancellation)
            .await
            .unwrap_err();

        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert_eq!(error.message, "tool invocation cancelled");
    }

    fn byte_session(stdin: bool) -> (StartedPublicAgentSession, InvocationAccepted) {
        use golem_api_grpc::proto::golem::worker::{
            DurableStreamHandle, DurableStreamMapping, StreamInvocationIdentity, StreamMappingRole,
        };
        let mut session = empty_session();
        let Some(invocation_request::Request::Start(start)) =
            session.initial_request.request.as_mut()
        else {
            unreachable!()
        };
        let tool = start.external_tool.as_mut().unwrap();
        tool.stdin = stdin;
        tool.stdout = true;
        let uuid = |n| golem_api_grpc::proto::golem::common::Uuid {
            high_bits: 0,
            low_bits: n,
        };
        let agent = Some(session.agent_id.clone().into());
        let environment = Some(golem_api_grpc::proto::golem::common::EnvironmentId {
            value: Some(uuid(3)),
        });
        let mapping = |id, role| DurableStreamMapping {
            transport_stream_id: id,
            role: role as i32,
            handle: Some(DurableStreamHandle {
                format_version: 1,
                stream_id: Some(uuid(id)),
                producer_environment_id: environment,
                producer: agent.clone(),
                expected_producer_fingerprint: Some(uuid(4)),
                source_invocation: Some(StreamInvocationIdentity {
                    callee_environment_id: environment,
                    callee: agent.clone(),
                    callee_fingerprint: Some(uuid(4)),
                    idempotency_key: start.idempotency_key.clone(),
                }),
                component_revision: Some(1),
                element_schema_fingerprint: vec![5; 32],
            }),
            high_water: None,
        };
        let mut mappings = vec![mapping(71, StreamMappingRole::Output)];
        if stdin {
            mappings.push(mapping(70, StreamMappingRole::Input));
        }
        let accepted = InvocationAccepted {
            agent_id: agent,
            environment_id: environment,
            idempotency_key: start.idempotency_key.clone(),
            component_revision: Some(1),
            attachment_id: Some(uuid(1)),
            attempt_id: Some(uuid(2)),
            callee_fingerprint: Some(uuid(4)),
            epoch: 1,
            tool_name: Some("tool".to_string()),
            tool_stdin_stream_id: stdin.then_some(70),
            tool_stdout_stream_id: Some(71),
            stream_mappings: mappings,
            ..Default::default()
        };
        (session, accepted)
    }

    fn durable_offset(sequence: u64) -> Vec<u8> {
        let mut offset = vec![0; 24];
        offset[0] = 1;
        offset[8..16].copy_from_slice(&sequence.to_be_bytes());
        offset
    }

    fn output_frame(bytes: Vec<u8>, sequence: u64) -> invocation_response::Response {
        invocation_response::Response::OutputItem(
            golem_api_grpc::proto::golem::worker::OutputStreamItem {
                transport_stream_id: 71,
                producer_sequence: sequence,
                durable_stream_id: Some(golem_api_grpc::proto::golem::common::Uuid {
                    high_bits: 0,
                    low_bits: 71,
                }),
                durable_offset: durable_offset(sequence),
                epoch: 1,
                logical_item_count: bytes.len() as u64,
                packed_u8: bytes,
                ..Default::default()
            },
        )
    }

    #[test]
    #[timeout("10s")]
    async fn stdout_is_drained_before_stdin_and_after_structured_result() {
        use golem_api_grpc::proto::golem::worker::{
            InvocationResponse, InvocationSessionCompletion, InvocationSessionResult,
            OutputStreamEnd,
        };
        let (mut session, accepted) = byte_session(true);
        let (requests, mut input) = mpsc::channel::<InvocationRequest>(1);
        let (output, responses) = mpsc::channel(1);
        session.responses = Box::pin(ReceiverStream::new(responses));
        let expected_input = vec![137; INPUT_CHUNK * 3 + 17];
        let expected_copy = expected_input.clone();
        let producer = async move {
            let send = async |response| {
                output
                    .send(Ok(InvocationResponse {
                        response: Some(response),
                    }))
                    .await
                    .unwrap()
            };
            send(invocation_response::Response::Accepted(accepted.clone())).await;
            // More than the bounded channel can hold, before reading any stdin.
            for sequence in 0..4 {
                send(output_frame(vec![sequence as u8 + 9], sequence)).await;
            }
            let mut received = Vec::new();
            loop {
                match input.recv().await.unwrap().request.unwrap() {
                    invocation_request::Request::InputItem(item) => {
                        assert_eq!(item.sequence, received.len() as u64);
                        let Some(input_stream_item::Payload::PackedU8(bytes)) = item.payload else {
                            panic!("not bytes")
                        };
                        let count = bytes.len() as u64;
                        received.extend(bytes);
                        send(invocation_response::Response::InputAck(
                            golem_api_grpc::proto::golem::worker::InputStreamAck {
                                transport_stream_id: 70,
                                highest_contiguous_sequence: received.len() as u64 - 1,
                                logical_item_count: count,
                                durable_stream_id: item.durable_stream_id,
                                resulting_offset: durable_offset(received.len() as u64),
                                epoch: 1,
                                ..Default::default()
                            },
                        ))
                        .await;
                    }
                    invocation_request::Request::InputEnd(end) => {
                        assert_eq!(end.sequence, received.len() as u64);
                        send(invocation_response::Response::InputAck(
                            golem_api_grpc::proto::golem::worker::InputStreamAck {
                                transport_stream_id: 70,
                                highest_contiguous_sequence: end.sequence,
                                logical_item_count: 1,
                                durable_stream_id: end.durable_stream_id,
                                resulting_offset: durable_offset(end.sequence + 1),
                                epoch: 1,
                                ..Default::default()
                            },
                        ))
                        .await;
                        break;
                    }
                    _ => panic!("unexpected input frame"),
                }
            }
            assert_eq!(received, expected_copy);
            send(invocation_response::Response::Result(
                InvocationSessionResult {
                    agent_id: accepted.agent_id,
                    component_revision: accepted.component_revision,
                    idempotency_key: accepted.idempotency_key,
                    result: Some(invocation_session_result::Result::ToolResult(
                        golem_api_grpc::proto::golem::worker::PublicExternalToolResult {
                            result: Some(golem_api_grpc::proto::golem::worker::public_external_tool_result::Result::Success(
                                golem_api_grpc::proto::golem::worker::PublicToolInvocationResult { result: None }
                            )),
                        },
                    )),
                    ..Default::default()
                },
            ))
            .await;
            send(output_frame(vec![99], 4)).await;
            send(invocation_response::Response::OutputEnd(OutputStreamEnd {
                transport_stream_id: 71,
                producer_sequence: 5,
                durable_stream_id: Some(golem_api_grpc::proto::golem::common::Uuid {
                    high_bits: 0,
                    low_bits: 71,
                }),
                durable_offset: durable_offset(5),
                epoch: 1,
            }))
            .await;
            send(invocation_response::Response::Finished(
                InvocationSessionCompletion {
                    outcome: Some(invocation_session_completion::Outcome::Success(
                        Default::default(),
                    )),
                },
            ))
            .await;
        };
        let (result, ()) = tokio::join!(
            collect_session(
                session,
                requests,
                Some(expected_input),
                true,
                CancellationToken::new()
            ),
            producer
        );
        let (result, bytes) = result.unwrap();
        assert!(matches!(result, PublicExternalToolResult::Success(_)));
        assert_eq!(bytes, [9, 10, 11, 12, 99]);
    }

    #[test]
    #[timeout("5s")]
    async fn stdout_over_limit_is_rejected_and_consumer_cancelled() {
        use golem_api_grpc::proto::golem::worker::InvocationResponse;
        let (mut session, accepted) = byte_session(false);
        let mut frames = vec![invocation_response::Response::Accepted(accepted)];
        for sequence in (0..MAX_OUTPUT).step_by(INPUT_CHUNK) {
            frames.push(output_frame(vec![1; INPUT_CHUNK], sequence as u64));
        }
        frames.push(output_frame(vec![2], MAX_OUTPUT as u64));
        session.responses = Box::pin(stream::iter(frames.into_iter().map(|response| {
            Ok(InvocationResponse {
                response: Some(response),
            })
        })));
        let (sender, mut receiver) = mpsc::channel(2);
        let error = collect_session(session, sender, None, true, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.message.contains("16 MiB"), "{error:?}");
        let invocation_request::Request::StreamCancel(cancel) =
            receiver.recv().await.unwrap().request.unwrap()
        else {
            panic!("expected cancellation")
        };
        assert_eq!(cancel.transport_stream_id, 71);
        assert_eq!(cancel.role, StreamCancelRole::OutputConsumer as i32);
    }

    #[test]
    fn native_results_match_mcp_json_and_content_contracts() {
        use golem_common::model::tool::SerializableToolInvocationResult;
        use golem_common::schema::tool::{
            CommandAnnotations, CommandBody, CommandNode, CommandTree, Doc, Formatter, ResultSpec,
            StreamSpec, Tool as NativeTool,
        };
        let definition = NativeTool {
            version: "1.0.0".to_string(),
            schema: SchemaGraph::empty(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: "test".to_string(),
                    aliases: vec![],
                    doc: Doc::default(),
                    globals: Default::default(),
                    subcommands: vec![],
                    body: Some(CommandBody {
                        positionals: Default::default(),
                        options: vec![],
                        flags: vec![],
                        constraints: vec![],
                        stdin: None,
                        stdout: Some(StreamSpec {
                            doc: Doc::default(),
                            mime: vec!["image/png".to_string()],
                            required: false,
                        }),
                        result: Some(ResultSpec {
                            type_: SchemaType::option(SchemaType::string()),
                            doc: Doc::default(),
                            formatters: vec![Formatter {
                                name: "json".to_string(),
                                doc: Doc::default(),
                            }],
                            default_formatter: "json".to_string(),
                        }),
                        errors: vec![],
                        annotations: Some(CommandAnnotations {
                            read_only: true,
                            destructive: false,
                            idempotent: false,
                            open_world: true,
                        }),
                    }),
                }],
            },
        };
        let export = golem_service_base::mcp::native_tool::compile_native_tool_exports(
            ComponentId::new(),
            "test:owner".try_into().unwrap(),
            "test".try_into().unwrap(),
            &definition,
            None,
            None,
        )
        .unwrap()
        .remove(0);
        let value = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::option(SchemaType::string())),
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String("hello".to_string()))),
            },
        );
        let result = project_result(
            &export,
            PublicExternalToolResult::Success(SerializableToolInvocationResult {
                result: Some(value.clone()),
            }),
            vec![0, 255, 17],
        )
        .unwrap();
        assert_eq!(result.structured_content, Some(json!({"value":"hello"})));
        let content = serde_json::to_value(&result.content).unwrap();
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["data"], "AP8R");
        assert_eq!(content[1]["mimeType"], "image/png");
        let hints = tool_metadata(&export).unwrap().annotations.unwrap();
        assert_eq!(
            (
                hints.read_only_hint,
                hints.destructive_hint,
                hints.idempotent_hint,
                hints.open_world_hint
            ),
            (Some(true), Some(false), Some(false), Some(true))
        );
        let custom = project_result(
            &export,
            PublicExternalToolResult::Failure(SerializableToolRpcError::RemoteToolError(Box::new(
                SerializableToolError::CustomError(Box::new(
                    golem_common::model::tool::SerializableCustomToolError {
                        name: "example-error".to_string(),
                        payload: value,
                    },
                )),
            ))),
            vec![],
        )
        .unwrap();
        assert_eq!(custom.is_error, Some(true));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                serde_json::to_value(custom.content).unwrap()[0]["text"]
                    .as_str()
                    .unwrap()
            )
            .unwrap(),
            json!({"name": "example-error", "payload": "hello"})
        );
        assert_eq!(
            project_result(
                &export,
                PublicExternalToolResult::Failure(SerializableToolRpcError::RemoteInternalError(
                    "execution failed".into()
                )),
                vec![]
            )
            .unwrap()
            .is_error,
            Some(true)
        );
        assert_eq!(
            project_result(
                &export,
                PublicExternalToolResult::Failure(SerializableToolRpcError::RemoteToolError(
                    Box::new(SerializableToolError::InvalidInput("bad argument".into()))
                )),
                vec![]
            )
            .unwrap_err()
            .code,
            rmcp::model::ErrorCode::INVALID_PARAMS
        );
    }
}
