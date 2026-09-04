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

use futures::{SinkExt, StreamExt};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::{
    InvocationRejectionReason, InvocationRequest, InvocationStart,
    ResumeAttach as PrivateResumeAttach, ResumeOperation as PrivateResumeOperation,
    invocation_request, invocation_response, invocation_session_completion,
    invocation_session_result,
};
use golem_client::model::ComponentDto;
use golem_common::base_model::durable_stream::AttachmentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_session_public::{
    DecimalU64, INVOCATION_SESSION_SUBPROTOCOL, INVOCATION_SESSION_VERSION, InvocationSelector,
    PublicClientMessage, PublicErrorCode, PublicInvocationOutcome, PublicInvocationResult,
    PublicResumeOperation, PublicServerMessage, PublicStreamDirection, decode_server_text,
    encode_text,
};
use golem_common::model::{AgentId, IdempotencyKey, RoutingTable};
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_common::{agent_id, data_value};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, SEC_WEBSOCKET_PROTOCOL};
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

inherit_test_dep!(EnvBasedTestDependencies);

type PublicInvocationSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_public_invocation_socket(
    deps: &EnvBasedTestDependencies,
    token: Option<&TokenSecret>,
) -> Result<PublicInvocationSocket, WebSocketError> {
    connect_public_invocation_socket_with_subprotocol(
        deps,
        token,
        Some(INVOCATION_SESSION_SUBPROTOCOL),
    )
    .await
}

async fn connect_public_invocation_socket_with_subprotocol(
    deps: &EnvBasedTestDependencies,
    token: Option<&TokenSecret>,
    subprotocol: Option<&str>,
) -> Result<PublicInvocationSocket, WebSocketError> {
    let worker_service = deps.worker_service();
    let url = format!(
        "ws://{}:{}/v1/agents/invoke-agent-session",
        worker_service.http_host(),
        worker_service.http_port()
    );
    let mut request = url.into_client_request()?;
    if let Some(token) = token {
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {}", token.secret()).parse().unwrap(),
        );
    }
    if let Some(subprotocol) = subprotocol {
        request
            .headers_mut()
            .insert(SEC_WEBSOCKET_PROTOCOL, subprotocol.parse().unwrap());
    }
    let (socket, response) = tokio_tungstenite::connect_async(request).await?;
    if subprotocol == Some(INVOCATION_SESSION_SUBPROTOCOL) {
        assert_eq!(
            response
                .headers()
                .get(SEC_WEBSOCKET_PROTOCOL)
                .and_then(|value| value.to_str().ok()),
            Some(INVOCATION_SESSION_SUBPROTOCOL)
        );
    }
    Ok(socket)
}

async fn send_public_request(
    socket: &mut PublicInvocationSocket,
    request: &PublicClientMessage,
) -> anyhow::Result<()> {
    socket
        .send(Message::Text(encode_text(request)?.into()))
        .await?;
    Ok(())
}

async fn receive_public_response(
    socket: &mut PublicInvocationSocket,
) -> anyhow::Result<PublicServerMessage> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                return decode_server_text(text.as_bytes()).map_err(Into::into);
            }
            Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await?,
            Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => {}
            Some(Ok(Message::Binary(_))) => {
                anyhow::bail!("public invocation returned an unexpected binary frame")
            }
            Some(Ok(Message::Close(close))) => {
                anyhow::bail!("public invocation closed before its next response: {close:?}")
            }
            Some(Err(error)) => return Err(error.into()),
            None => anyhow::bail!("public invocation connection ended before its next response"),
        }
    }
}

fn public_start(
    application_name: &str,
    environment_name: &str,
    agent_name: &str,
    method_name: &str,
    method_parameters: serde_json::Value,
) -> PublicClientMessage {
    PublicClientMessage::InvocationStart {
        attempt_id: uuid::Uuid::new_v4(),
        config: Vec::new(),
        idempotency_key: IdempotencyKey::fresh().value,
        method_parameters,
        selector: Box::new(InvocationSelector {
            agent_type: "StreamingRpcTarget".to_string(),
            application: application_name.to_string(),
            constructor_parameters: serde_json::json!({ "name": agent_name }),
            environment: environment_name.to_string(),
            method: method_name.to_string(),
            phantom_id: None,
        }),
        version: INVOCATION_SESSION_VERSION,
    }
}

async fn run_public_session(
    deps: &EnvBasedTestDependencies,
    token: &TokenSecret,
    start: PublicClientMessage,
) -> anyhow::Result<Vec<PublicServerMessage>> {
    let mut socket = connect_public_invocation_socket(deps, Some(token)).await?;
    send_public_request(&mut socket, &start).await?;

    let mut responses = Vec::new();
    loop {
        let response = receive_public_response(&mut socket).await?;
        let terminal = matches!(
            response,
            PublicServerMessage::InvocationRejected { .. }
                | PublicServerMessage::InvocationFinished { .. }
        );
        responses.push(response);
        if terminal {
            break;
        }
    }
    match socket.next().await {
        Some(Ok(Message::Close(_))) => {}
        other => anyhow::bail!("completed public invocation must close cleanly, got {other:?}"),
    }
    Ok(responses)
}

async fn resume_public_input_with_end(
    deps: &EnvBasedTestDependencies,
    token: &TokenSecret,
    session_token: String,
    input_channel: u32,
) -> anyhow::Result<()> {
    let resume = PublicClientMessage::ResumeAttach {
        attempt_id: uuid::Uuid::new_v4(),
        operation: PublicResumeOperation::Resume,
        output_cursors: Vec::new(),
        session_token,
        version: INVOCATION_SESSION_VERSION,
    };
    let mut socket = connect_public_invocation_socket(deps, Some(token)).await?;
    send_public_request(&mut socket, &resume).await?;
    let resumed = receive_public_response(&mut socket).await?;
    let resumed_channel = match resumed {
        PublicServerMessage::InvocationAccepted { mappings, .. } => mappings
            .into_iter()
            .find(|mapping| {
                mapping.direction == PublicStreamDirection::Input
                    && mapping.input_high_water.as_ref().is_some_and(|high_water| {
                        high_water.sequence == DecimalU64(0) && !high_water.terminal
                    })
            })
            .map(|mapping| mapping.channel)
            .ok_or_else(|| anyhow::anyhow!("resume omitted its fresh input channel"))?,
        other => anyhow::bail!("detached public invocation did not resume: {other:?}"),
    };
    assert_ne!(resumed_channel, input_channel);
    let end = PublicClientMessage::InputStreamEnd {
        channel: resumed_channel,
        sequence: DecimalU64(0),
        version: INVOCATION_SESSION_VERSION,
    };
    send_public_request(&mut socket, &end).await?;
    loop {
        let response = receive_public_response(&mut socket).await?;
        if matches!(response, PublicServerMessage::InvocationFinished { .. }) {
            break;
        }
    }
    Ok(())
}

fn cross_executor_agent_name(
    component: &ComponentDto,
    routing_table: &RoutingTable,
) -> anyhow::Result<String> {
    for index in 0..10_000 {
        let name = format!("generated-streaming-rpc-cross-{index}");
        let caller = agent_id!("StreamingRpcCaller", name.clone());
        let target = agent_id!("StreamingRpcTarget", name.clone());
        let caller = AgentId::from_agent_id(component.id, &caller)
            .map_err(|error| anyhow::anyhow!("invalid caller agent id: {error}"))?;
        let target = AgentId::from_agent_id(component.id, &target)
            .map_err(|error| anyhow::anyhow!("invalid target agent id: {error}"))?;
        let caller_pod = routing_table
            .lookup(&caller)
            .ok_or_else(|| anyhow::anyhow!("caller agent has no executor assignment"))?;
        let target_pod = routing_table
            .lookup(&target)
            .ok_or_else(|| anyhow::anyhow!("target agent has no executor assignment"))?;
        if caller_pod != target_pod {
            return Ok(name);
        }
    }

    anyhow::bail!("could not find caller and target agent IDs assigned to different executors")
}

async fn invoke_agent_session(
    deps: &EnvBasedTestDependencies,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    method_name: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<Result<SchemaValue, String>> {
    let agent_id = AgentId::from_agent_id(component.id, agent_id)
        .map_err(|error| anyhow::anyhow!("invalid agent id: {error}"))?;
    let (_, input) = params.into_parts();
    let input = input.try_into().map_err(anyhow::Error::msg)?;
    let (frames, receiver) = mpsc::channel(8);
    let request = InvocationRequest {
        request: Some(invocation_request::Request::Start(InvocationStart {
            agent_id: Some(agent_id.into()),
            method_name: Some(method_name.to_string()),
            input: Some(input),
            idempotency_key: Some(IdempotencyKey::fresh().into()),
            context: None,
            auth_ctx: Some(AuthCtx::System.into()),
            principal: None,
            environment_id: None,
            config: Vec::new(),
            component_owner_account_id: None,
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            schedule_at: None,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: None,
            expected_callee_fingerprint: None,
            durable_input_mappings: Vec::new(),
            scope_card: None,
        })),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&request)
        .map_err(anyhow::Error::msg)?;
    frames.send(request).await?;

    let worker_service = deps.worker_service();
    let mut client = WorkerServiceClient::connect(format!(
        "http://{}:{}",
        worker_service.grpc_host(),
        worker_service.gprc_port()
    ))
    .await?;
    let mut inbound = client
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let mut result = None;
    let mut terminal = None;
    while let Some(response) = inbound.message().await? {
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        match response.response {
            Some(invocation_response::Response::Accepted(_)) => {}
            Some(invocation_response::Response::Rejected(rejected)) => {
                terminal = Some(Err(rejected.error));
            }
            Some(invocation_response::Response::Result(value)) => {
                if result.is_some() {
                    anyhow::bail!("invocation session returned more than one result");
                }
                result = match value.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => {
                        Some(value.try_into().map_err(anyhow::Error::msg)?)
                    }
                    Some(invocation_session_result::Result::NoResult(_)) | None => {
                        anyhow::bail!("invocation session returned no method result")
                    }
                };
            }
            Some(invocation_response::Response::Finished(finished)) => {
                terminal = Some(match finished.outcome {
                    Some(invocation_session_completion::Outcome::Success(_)) => {
                        result.take().map(Ok).ok_or_else(|| {
                            anyhow::anyhow!("invocation session ended without a result")
                        })?
                    }
                    Some(invocation_session_completion::Outcome::Failure(failure)) => {
                        Err(failure.message)
                    }
                    None => anyhow::bail!("invocation session completion has no outcome"),
                });
            }
            Some(other) => {
                anyhow::bail!("unexpected outer invocation session frame: {other:?}")
            }
            None => anyhow::bail!("empty outer invocation session frame"),
        }
    }
    terminal.ok_or_else(|| anyhow::anyhow!("invocation session response ended before completion"))
}

fn assert_streaming_report(value: SchemaValue) {
    assert_eq!(
        value,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::U32(1),
                        SchemaValue::U32(2),
                        SchemaValue::U32(3),
                    ],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::U32(4),
                        SchemaValue::U32(5),
                        SchemaValue::U32(6),
                    ],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::U32(70),
                        SchemaValue::U32(80),
                        SchemaValue::U32(90),
                    ],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::String("left".to_string()),
                        SchemaValue::String("right".to_string()),
                    ],
                },
                SchemaValue::List {
                    elements: vec![SchemaValue::U32(10), SchemaValue::U32(11)],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::String("first".to_string()),
                        SchemaValue::String("second".to_string()),
                    ],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::List {
                            elements: vec![SchemaValue::U32(1), SchemaValue::U32(2)],
                        },
                        SchemaValue::List {
                            elements: vec![
                                SchemaValue::U32(3),
                                SchemaValue::U32(4),
                                SchemaValue::U32(5),
                            ],
                        },
                    ],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::String("a".to_string()),
                        SchemaValue::String("b".to_string()),
                    ],
                },
                SchemaValue::List {
                    elements: (0..64).map(SchemaValue::U32).collect(),
                },
                SchemaValue::U64(42),
            ],
        }
    );
}

#[test]
#[timeout("4 minutes")]
#[tracing::instrument]
async fn grpc_invocation_session_routes_resume_and_takeover(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let environment_id = EnvironmentId(uuid::Uuid::new_v4());
    let agent_id = AgentId {
        component_id: ComponentId(uuid::Uuid::new_v4()),
        agent_id: "missing-resume-target".to_string(),
    };
    let worker_service = deps.worker_service();
    let mut client = WorkerServiceClient::connect(format!(
        "http://{}:{}",
        worker_service.grpc_host(),
        worker_service.gprc_port()
    ))
    .await?;

    for operation in [
        PrivateResumeOperation::Resume,
        PrivateResumeOperation::Takeover,
    ] {
        let idempotency_key = IdempotencyKey::fresh();
        let request = InvocationRequest {
            request: Some(invocation_request::Request::ResumeAttach(
                PrivateResumeAttach {
                    idempotency_key: Some(idempotency_key.clone().into()),
                    agent_id: Some(agent_id.clone().into()),
                    environment_id: Some(environment_id.into()),
                    attachment_id: Some(
                        AttachmentId::primary(environment_id, &agent_id, &idempotency_key)?
                            .0
                            .into(),
                    ),
                    attempt_id: Some(uuid::Uuid::new_v4().into()),
                    expected_callee_fingerprint: Some(uuid::Uuid::new_v4().into()),
                    expected_epoch: 1,
                    operation: operation as i32,
                    cursors: Vec::new(),
                    auth_ctx: Some(AuthCtx::System.into()),
                    principal: Some(Default::default()),
                },
            )),
        };
        let mut state = InvocationSessionState::default();
        state
            .validate_trusted_request(&request)
            .map_err(anyhow::Error::msg)?;
        let (requests, receiver) = mpsc::channel(1);
        requests.send(request).await?;
        drop(requests);
        let mut responses = client
            .invoke_agent_session(ReceiverStream::new(receiver))
            .await?
            .into_inner();
        let response = responses
            .message()
            .await?
            .ok_or_else(|| anyhow::anyhow!("resume endpoint returned no response"))?;
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        let rejected = match response.response {
            Some(invocation_response::Response::Rejected(rejected)) => rejected,
            other => anyhow::bail!("resume endpoint returned an unexpected response: {other:?}"),
        };
        assert_eq!(
            rejected.reason,
            InvocationRejectionReason::Internal as i32,
            "resume endpoint returned an unexpected rejection: {rejected:?}"
        );
        assert_eq!(rejected.error, "Component not found");
        assert!(responses.message().await?.is_none());
        assert!(state.is_complete());
    }
    Ok(())
}

#[test]
#[timeout("4 minutes")]
#[tracing::instrument]
async fn generated_rust_client_streaming_rpc_cross_executor(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, environment) = user.app_and_env().await?;
    let component = user
        .component(&environment.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;
    let routing_table = deps.shard_manager().get_routing_table().await?;
    let name = cross_executor_agent_name(&component, &routing_table)?;
    let caller_agent_id = agent_id!("StreamingRpcCaller", name);

    let result = invoke_agent_session(deps, &component, &caller_agent_id, "run", data_value!())
        .await?
        .map_err(anyhow::Error::msg)?;
    assert_streaming_report(result);

    let producer_error = invoke_agent_session(
        deps,
        &component,
        &caller_agent_id,
        "call_producer_error",
        data_value!(),
    )
    .await?
    .expect_err("producer stream error must fail the invocation session");
    assert!(
        producer_error.contains("Component trapped")
            || producer_error.contains("value-node index out of range: 0"),
        "unexpected producer error: {producer_error}"
    );

    let stream_free_caller_id = agent_id!("StreamingRpcCaller", "stream-free-after-stream-error");
    let first = user
        .invoke_and_await_agent(
            &component,
            &stream_free_caller_id,
            "call_stream_free",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    let second = user
        .invoke_and_await_agent(
            &component,
            &stream_free_caller_id,
            "call_stream_free",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    assert_eq!((first, second), (1, 2));
    Ok(())
}

#[test]
#[timeout("4 minutes")]
#[tracing::instrument]
async fn public_websocket_invocation_forwards_scalar_and_streaming_sessions(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (application, environment) = user.app_and_env().await?;
    user.component(&environment.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;
    let application_name = &application.name.0;
    let environment_name = &environment.name.0;
    let agent_name = format!("public-streaming-{}", uuid::Uuid::new_v4());

    let scalar = run_public_session(
        deps,
        &user.token,
        public_start(
            application_name,
            environment_name,
            &agent_name,
            "ping",
            serde_json::json!({}),
        ),
    )
    .await?;
    assert_eq!(scalar.len(), 3);
    assert!(matches!(
        scalar[0],
        PublicServerMessage::InvocationAccepted { .. }
    ));
    let PublicServerMessage::InvocationResult {
        mappings,
        result: PublicInvocationResult::Value { value },
        ..
    } = &scalar[1]
    else {
        anyhow::bail!("scalar public invocation did not return a result")
    };
    assert!(mappings.is_empty());
    assert_eq!(value, &serde_json::json!("42"));
    assert!(matches!(
        scalar[2],
        PublicServerMessage::InvocationFinished {
            outcome: PublicInvocationOutcome::Success,
            ..
        }
    ));

    let produced = run_public_session(
        deps,
        &user.token,
        public_start(
            application_name,
            environment_name,
            &agent_name,
            "produce",
            serde_json::json!({ "values": [3, 5, 8] }),
        ),
    )
    .await?;
    assert_eq!(produced.len(), 7);
    let PublicServerMessage::InvocationResult {
        mappings,
        result: PublicInvocationResult::Value { value },
        ..
    } = &produced[1]
    else {
        anyhow::bail!("streaming public invocation did not return an initial result")
    };
    let [output_mapping] = mappings.as_slice() else {
        anyhow::bail!("streaming result did not expose exactly one output stream")
    };
    assert_eq!(output_mapping.direction, PublicStreamDirection::Output);
    assert_eq!(
        value,
        &serde_json::json!({
            "$stream": { "streamToken": output_mapping.stream_token.clone() }
        })
    );
    let items = produced[2..5]
        .iter()
        .enumerate()
        .map(|(expected_sequence, response)| match response {
            PublicServerMessage::OutputStreamItem {
                channel,
                sequence,
                value,
                ..
            } => {
                assert_eq!(*channel, output_mapping.channel);
                assert_eq!(*sequence, DecimalU64(expected_sequence as u64));
                value.clone()
            }
            other => panic!("expected output item, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        items,
        vec![
            serde_json::json!(3),
            serde_json::json!(5),
            serde_json::json!(8)
        ]
    );
    assert!(matches!(
        produced[5],
        PublicServerMessage::OutputStreamEnd {
            channel,
            sequence: DecimalU64(3),
            ..
        } if channel == output_mapping.channel
    ));
    assert!(matches!(
        produced[6],
        PublicServerMessage::InvocationFinished {
            outcome: PublicInvocationOutcome::Success,
            ..
        }
    ));

    let input_reference = uuid::Uuid::new_v4();
    let start = public_start(
        application_name,
        environment_name,
        &agent_name,
        "consume",
        serde_json::json!({
            "input": { "$stream": { "provisionalRef": input_reference } }
        }),
    );
    let mut socket = connect_public_invocation_socket(deps, Some(&user.token)).await?;
    send_public_request(&mut socket, &start).await?;
    let accepted = receive_public_response(&mut socket).await?;
    let input_channel = match accepted {
        PublicServerMessage::InvocationAccepted { mappings, .. } => mappings
            .iter()
            .find(|mapping| mapping.provisional_ref == Some(input_reference))
            .map(|mapping| mapping.channel)
            .ok_or_else(|| anyhow::anyhow!("acceptance omitted the input stream mapping"))?,
        other => anyhow::bail!("streaming input was not accepted: {other:?}"),
    };

    for (sequence, value) in [13_u32, 21].into_iter().enumerate() {
        let request = PublicClientMessage::InputStreamItem {
            channel: input_channel,
            sequence: DecimalU64(sequence as u64),
            value: serde_json::json!(value),
            version: INVOCATION_SESSION_VERSION,
        };
        send_public_request(&mut socket, &request).await?;
        let ack = receive_public_response(&mut socket).await?;
        assert!(matches!(
            ack,
            PublicServerMessage::InputStreamAck {
                channel,
                highest_contiguous_sequence,
                terminal: false,
                ..
            } if channel == input_channel
                && highest_contiguous_sequence == DecimalU64(sequence as u64 + 1)
        ));
    }
    let end = PublicClientMessage::InputStreamEnd {
        channel: input_channel,
        sequence: DecimalU64(2),
        version: INVOCATION_SESSION_VERSION,
    };
    send_public_request(&mut socket, &end).await?;
    let mut consumed = None;
    loop {
        let response = receive_public_response(&mut socket).await?;
        match response {
            PublicServerMessage::InputStreamAck {
                channel,
                highest_contiguous_sequence: DecimalU64(2),
                terminal: true,
                ..
            } => assert_eq!(channel, input_channel),
            PublicServerMessage::InvocationResult {
                result: PublicInvocationResult::Value { value },
                ..
            } => consumed = Some(value),
            PublicServerMessage::InvocationFinished {
                outcome: PublicInvocationOutcome::Success,
                ..
            } => break,
            PublicServerMessage::InvocationRejected { code, message, .. } => {
                anyhow::bail!("streaming input was rejected ({code:?}): {message}")
            }
            _ => {}
        }
    }
    assert_eq!(consumed, Some(serde_json::json!([13, 21])));

    let blocked_reference = uuid::Uuid::new_v4();
    let blocked = public_start(
        application_name,
        environment_name,
        &agent_name,
        "consume",
        serde_json::json!({
            "input": { "$stream": { "provisionalRef": blocked_reference } }
        }),
    );
    let mut blocked_socket = connect_public_invocation_socket(deps, Some(&user.token)).await?;
    send_public_request(&mut blocked_socket, &blocked).await?;
    let accepted = receive_public_response(&mut blocked_socket).await?;
    let (blocked_session_token, blocked_channel) = match accepted {
        PublicServerMessage::InvocationAccepted {
            mappings,
            session_token,
            ..
        } => {
            let channel = mappings
                .iter()
                .find(|mapping| mapping.provisional_ref == Some(blocked_reference))
                .map(|mapping| mapping.channel)
                .ok_or_else(|| anyhow::anyhow!("blocked input mapping was omitted"))?;
            (session_token, channel)
        }
        other => anyhow::bail!("blocked public invocation was not accepted: {other:?}"),
    };
    blocked_socket.close(None).await?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(message) = blocked_socket.next().await {
            match message {
                Ok(Message::Close(_)) | Err(_) => return Ok::<_, anyhow::Error>(()),
                Ok(_) => {}
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("blocked invocation close handshake timed out"))??;
    drop(blocked_socket);

    resume_public_input_with_end(deps, &user.token, blocked_session_token, blocked_channel).await?;

    let subsequent = run_public_session(
        deps,
        &user.token,
        public_start(
            application_name,
            environment_name,
            &agent_name,
            "ping",
            serde_json::json!({}),
        ),
    )
    .await?;
    assert!(subsequent.iter().any(|response| matches!(
        response,
        PublicServerMessage::InvocationFinished {
            outcome: PublicInvocationOutcome::Success,
            ..
        }
    )));

    let capability_reference = uuid::Uuid::new_v4();
    let mut capability_socket = connect_public_invocation_socket(deps, Some(&user.token)).await?;
    send_public_request(
        &mut capability_socket,
        &public_start(
            application_name,
            environment_name,
            &agent_name,
            "consume",
            serde_json::json!({
                "input": { "$stream": { "provisionalRef": capability_reference } }
            }),
        ),
    )
    .await?;
    let accepted = receive_public_response(&mut capability_socket).await?;
    let (capability_session_token, capability_channel) = match accepted {
        PublicServerMessage::InvocationAccepted {
            mappings,
            session_token,
            ..
        } => {
            let channel = mappings
                .iter()
                .find(|mapping| mapping.provisional_ref == Some(capability_reference))
                .map(|mapping| mapping.channel)
                .ok_or_else(|| anyhow::anyhow!("capability input mapping was omitted"))?;
            (session_token, channel)
        }
        other => anyhow::bail!("capability invocation was not accepted: {other:?}"),
    };
    send_public_request(
        &mut capability_socket,
        &PublicClientMessage::InputStreamItem {
            channel: capability_channel,
            sequence: DecimalU64(0),
            value: serde_json::json!({ "$secret": { "token": "forged" } }),
            version: INVOCATION_SESSION_VERSION,
        },
    )
    .await?;
    let close = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match capability_socket.next().await {
                Some(Ok(Message::Close(close))) => break Ok(close),
                Some(Ok(_)) => {}
                Some(Err(error)) => break Err(anyhow::Error::from(error)),
                None => anyhow::bail!("capability injection ended without a protocol close"),
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("capability injection did not close the public session"))??;
    assert!(matches!(
        close.map(|frame| frame.code),
        Some(tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Protocol)
    ));
    drop(capability_socket);

    resume_public_input_with_end(
        deps,
        &user.token,
        capability_session_token,
        capability_channel,
    )
    .await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        run_public_session(
            deps,
            &user.token,
            public_start(
                application_name,
                environment_name,
                &agent_name,
                "ping",
                serde_json::json!({}),
            ),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("resumed capability rejection did not finish"))??;
    Ok(())
}

#[test]
#[timeout("2 minutes")]
#[tracing::instrument]
async fn public_websocket_invocation_enforces_auth_frames_and_rejections(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let missing_auth = connect_public_invocation_socket(deps, None)
        .await
        .expect_err("missing authentication must reject the WebSocket upgrade");
    assert!(matches!(
        missing_auth,
        WebSocketError::Http(response) if response.status().as_u16() == 401
    ));

    let invalid_token = TokenSecret::trusted("not-a-valid-token".to_string());
    let invalid_auth = connect_public_invocation_socket(deps, Some(&invalid_token))
        .await
        .expect_err("invalid authentication must reject the WebSocket upgrade");
    assert!(matches!(
        invalid_auth,
        WebSocketError::Http(response) if response.status().as_u16() == 401
    ));

    let user = deps.user().await?;
    for subprotocol in [None, Some("unsupported.invocation.v1")] {
        let error =
            connect_public_invocation_socket_with_subprotocol(deps, Some(&user.token), subprotocol)
                .await
                .expect_err("missing or unsupported subprotocol must reject the WebSocket upgrade");
        assert!(matches!(
            error,
            WebSocketError::Http(response) if response.status().as_u16() == 400
        ));
    }

    for invalid_message in [
        Message::Text("not json".into()),
        Message::Binary(vec![0x0a, 0x00].into()),
    ] {
        let mut socket = connect_public_invocation_socket(deps, Some(&user.token)).await?;
        socket.send(invalid_message).await?;
        let Some(Ok(Message::Close(Some(close)))) = socket.next().await else {
            anyhow::bail!("invalid public frame did not receive a close response")
        };
        assert!(matches!(
            close.code,
            tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Unsupported
                | tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Protocol
        ));
    }

    let rejected = run_public_session(
        deps,
        &user.token,
        public_start(
            "application-that-does-not-exist",
            "environment-that-does-not-exist",
            "missing-agent",
            "ping",
            serde_json::json!({}),
        ),
    )
    .await?;
    assert_eq!(rejected.len(), 1);
    let PublicServerMessage::InvocationRejected { code, message, .. } = &rejected[0] else {
        anyhow::bail!("unresolved public selector did not produce invocation-rejected")
    };
    assert_eq!(
        *code,
        PublicErrorCode::NotFound,
        "unexpected public rejection: {message}"
    );
    Ok(())
}
