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
use golem_api_grpc::proto::golem::schema::{
    RecordValue, SchemaValue as ProtoSchemaValue, SchemaValueStreamReference, SecretValue,
    schema_value,
};
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::{
    InputStreamEnd, InputStreamItem, InvocationAccepted, InvocationRejectionReason,
    InvocationRequest, InvocationResponse, InvocationStart, PublicInvocationRequest,
    PublicInvocationStart, ResumeAttach, ResumeOperation, input_stream_item, invocation_request,
    invocation_response, invocation_session_completion, invocation_session_result,
    public_invocation_request,
};
use golem_client::model::ComponentDto;
use golem_common::base_model::durable_stream::AttachmentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, IdempotencyKey, RoutingTable};
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_common::{agent_id, data_value};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use prost::Message as ProstMessage;
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

inherit_test_dep!(EnvBasedTestDependencies);

type PublicInvocationSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_public_invocation_socket(
    deps: &EnvBasedTestDependencies,
    token: Option<&TokenSecret>,
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
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(socket, _)| socket)
}

async fn send_public_request(
    socket: &mut PublicInvocationSocket,
    request: &PublicInvocationRequest,
) -> anyhow::Result<()> {
    socket
        .send(Message::Binary(request.encode_to_vec().into()))
        .await?;
    Ok(())
}

async fn receive_public_response(
    socket: &mut PublicInvocationSocket,
) -> anyhow::Result<InvocationResponse> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Binary(payload))) => {
                return InvocationResponse::decode(payload).map_err(Into::into);
            }
            Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await?,
            Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => {}
            Some(Ok(Message::Text(text))) => {
                anyhow::bail!("public invocation returned unexpected text frame: {text}")
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
    method_parameters: ProtoSchemaValue,
) -> PublicInvocationRequest {
    let constructor_parameters = SchemaValue::Record {
        fields: vec![SchemaValue::String(agent_name.to_string())],
    }
    .try_into()
    .unwrap();

    PublicInvocationRequest {
        request: Some(public_invocation_request::Request::Start(
            PublicInvocationStart {
                application_name: application_name.to_string(),
                environment_name: environment_name.to_string(),
                agent_type_name: "StreamingRpcTarget".to_string(),
                constructor_parameters: Some(constructor_parameters),
                phantom_id: None,
                config: Vec::new(),
                method_name: method_name.to_string(),
                method_parameters: Some(method_parameters),
                idempotency_key: Some(IdempotencyKey::fresh().into()),
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                expected_callee_fingerprint: None,
            },
        )),
    }
}

fn proto_record(fields: Vec<SchemaValue>) -> ProtoSchemaValue {
    SchemaValue::Record { fields }.try_into().unwrap()
}

async fn run_public_session(
    deps: &EnvBasedTestDependencies,
    token: &TokenSecret,
    start: PublicInvocationRequest,
) -> anyhow::Result<Vec<InvocationResponse>> {
    let mut socket = connect_public_invocation_socket(deps, Some(token)).await?;
    let mut state = InvocationSessionState::default();
    state
        .validate_public_request(&start)
        .map_err(anyhow::Error::msg)?;
    send_public_request(&mut socket, &start).await?;

    let mut responses = Vec::new();
    while !state.is_complete() {
        let response = receive_public_response(&mut socket).await?;
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        responses.push(response);
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
    acceptance: InvocationAccepted,
    transport_stream_id: u64,
) -> anyhow::Result<()> {
    let durable_stream_id = acceptance
        .stream_mappings
        .iter()
        .find(|mapping| mapping.transport_stream_id == transport_stream_id)
        .and_then(|mapping| mapping.handle.as_ref())
        .and_then(|handle| handle.stream_id)
        .ok_or_else(|| anyhow::anyhow!("acceptance omitted its input stream mapping"))?;
    let resume = PublicInvocationRequest {
        request: Some(public_invocation_request::Request::ResumeAttach(
            ResumeAttach {
                idempotency_key: acceptance.idempotency_key,
                agent_id: acceptance.agent_id,
                environment_id: acceptance.environment_id,
                attachment_id: acceptance.attachment_id,
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                expected_callee_fingerprint: acceptance.callee_fingerprint,
                expected_epoch: acceptance.epoch,
                operation: ResumeOperation::Resume as i32,
                cursors: Vec::new(),
                auth_ctx: None,
                principal: None,
            },
        )),
    };
    let mut socket = connect_public_invocation_socket(deps, Some(token)).await?;
    let mut state = InvocationSessionState::default();
    state
        .validate_public_request(&resume)
        .map_err(anyhow::Error::msg)?;
    send_public_request(&mut socket, &resume).await?;
    let resumed = receive_public_response(&mut socket).await?;
    state
        .validate_response(&resumed)
        .map_err(anyhow::Error::msg)?;
    let resumed_epoch = match resumed.response {
        Some(invocation_response::Response::Accepted(accepted)) => {
            assert_eq!(accepted.epoch, acceptance.epoch + 1);
            accepted.epoch
        }
        other => anyhow::bail!("detached public invocation did not resume: {other:?}"),
    };
    let end = PublicInvocationRequest {
        request: Some(public_invocation_request::Request::InputEnd(
            InputStreamEnd {
                transport_stream_id,
                sequence: 0,
                durable_stream_id: Some(durable_stream_id),
                epoch: resumed_epoch,
            },
        )),
    };
    state
        .validate_public_request(&end)
        .map_err(anyhow::Error::msg)?;
    send_public_request(&mut socket, &end).await?;
    while !state.is_complete() {
        let response = receive_public_response(&mut socket).await?;
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
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

    for operation in [ResumeOperation::Resume, ResumeOperation::Takeover] {
        let idempotency_key = IdempotencyKey::fresh();
        let request = InvocationRequest {
            request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
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
            })),
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
            proto_record(Vec::new()),
        ),
    )
    .await?;
    assert_eq!(scalar.len(), 3);
    assert!(matches!(
        scalar[0].response,
        Some(invocation_response::Response::Accepted(_))
    ));
    let Some(invocation_response::Response::Result(result)) = &scalar[1].response else {
        anyhow::bail!("scalar public invocation did not return a result")
    };
    let Some(invocation_session_result::Result::MethodResult(value)) = &result.result else {
        anyhow::bail!("scalar public invocation returned no method value")
    };
    assert_eq!(
        SchemaValue::try_from(value.clone()).map_err(anyhow::Error::msg)?,
        SchemaValue::U64(42)
    );
    assert!(matches!(
        scalar[2].response,
        Some(invocation_response::Response::Finished(_))
    ));

    let produced = run_public_session(
        deps,
        &user.token,
        public_start(
            application_name,
            environment_name,
            &agent_name,
            "produce",
            proto_record(vec![SchemaValue::List {
                elements: vec![
                    SchemaValue::U32(3),
                    SchemaValue::U32(5),
                    SchemaValue::U32(8),
                ],
            }]),
        ),
    )
    .await?;
    assert_eq!(produced.len(), 7);
    let Some(invocation_response::Response::Result(result)) = &produced[1].response else {
        anyhow::bail!("streaming public invocation did not return an initial result")
    };
    let Some(invocation_session_result::Result::MethodResult(result_value)) = &result.result else {
        anyhow::bail!("streaming public invocation returned no method value")
    };
    let Some(schema_value::Value::StreamReference(stream)) = &result_value.value else {
        anyhow::bail!("streaming public invocation did not return a stream reference")
    };
    let items = produced[2..5]
        .iter()
        .map(|response| match &response.response {
            Some(invocation_response::Response::OutputItem(item)) => {
                assert_eq!(item.transport_stream_id, stream.stream_id);
                SchemaValue::try_from(item.value.clone().unwrap()).unwrap()
            }
            other => panic!("expected output item, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        items,
        vec![
            SchemaValue::U32(3),
            SchemaValue::U32(5),
            SchemaValue::U32(8)
        ]
    );
    assert!(matches!(
        produced[5].response,
        Some(invocation_response::Response::OutputEnd(_))
    ));
    assert!(matches!(
        produced[6].response,
        Some(invocation_response::Response::Finished(_))
    ));

    let input_stream_id = 71;
    let stream_input = ProtoSchemaValue {
        value: Some(schema_value::Value::RecordValue(RecordValue {
            fields: vec![ProtoSchemaValue {
                value: Some(schema_value::Value::StreamReference(
                    SchemaValueStreamReference {
                        stream_id: input_stream_id,
                    },
                )),
            }],
        })),
    };
    let start = public_start(
        application_name,
        environment_name,
        &agent_name,
        "consume",
        stream_input,
    );
    let mut socket = connect_public_invocation_socket(deps, Some(&user.token)).await?;
    let mut state = InvocationSessionState::default();
    state
        .validate_public_request(&start)
        .map_err(anyhow::Error::msg)?;
    send_public_request(&mut socket, &start).await?;
    let accepted = receive_public_response(&mut socket).await?;
    state
        .validate_response(&accepted)
        .map_err(anyhow::Error::msg)?;
    let (input_durable_stream_id, input_epoch) = match accepted.response.as_ref() {
        Some(invocation_response::Response::Accepted(accepted)) => {
            let mapping = accepted
                .stream_mappings
                .iter()
                .find(|mapping| mapping.transport_stream_id == input_stream_id)
                .ok_or_else(|| anyhow::anyhow!("acceptance omitted the input stream mapping"))?;
            let stream_id = mapping
                .handle
                .as_ref()
                .and_then(|handle| handle.stream_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("input stream mapping omitted its durable stream ID")
                })?;
            (stream_id, accepted.epoch)
        }
        other => anyhow::bail!("streaming input was not accepted: {other:?}"),
    };

    for (sequence, value) in [13_u32, 21].into_iter().enumerate() {
        let request = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::InputItem(
                InputStreamItem {
                    transport_stream_id: input_stream_id,
                    sequence: sequence as u64,
                    payload: Some(input_stream_item::Payload::Value(
                        SchemaValue::U32(value)
                            .try_into()
                            .map_err(anyhow::Error::msg)?,
                    )),
                    durable_stream_id: Some(input_durable_stream_id),
                    epoch: input_epoch,
                },
            )),
        };
        state
            .validate_public_request(&request)
            .map_err(anyhow::Error::msg)?;
        send_public_request(&mut socket, &request).await?;
        let ack = receive_public_response(&mut socket).await?;
        state.validate_response(&ack).map_err(anyhow::Error::msg)?;
        assert!(matches!(
            ack.response,
            Some(invocation_response::Response::InputAck(_))
        ));
    }
    let end = PublicInvocationRequest {
        request: Some(public_invocation_request::Request::InputEnd(
            InputStreamEnd {
                transport_stream_id: input_stream_id,
                sequence: 2,
                durable_stream_id: Some(input_durable_stream_id),
                epoch: input_epoch,
            },
        )),
    };
    state
        .validate_public_request(&end)
        .map_err(anyhow::Error::msg)?;
    send_public_request(&mut socket, &end).await?;
    let mut consumed = None;
    while !state.is_complete() {
        let response = receive_public_response(&mut socket).await?;
        state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        if let Some(invocation_response::Response::Result(result)) = response.response
            && let Some(invocation_session_result::Result::MethodResult(value)) = result.result
        {
            consumed = Some(SchemaValue::try_from(value).map_err(anyhow::Error::msg)?);
        }
    }
    assert_eq!(
        consumed,
        Some(SchemaValue::List {
            elements: vec![SchemaValue::U32(13), SchemaValue::U32(21)],
        })
    );

    let blocked_stream_id = 73;
    let blocked = public_start(
        application_name,
        environment_name,
        &agent_name,
        "consume",
        ProtoSchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![ProtoSchemaValue {
                    value: Some(schema_value::Value::StreamReference(
                        SchemaValueStreamReference {
                            stream_id: blocked_stream_id,
                        },
                    )),
                }],
            })),
        },
    );
    let mut blocked_socket = connect_public_invocation_socket(deps, Some(&user.token)).await?;
    send_public_request(&mut blocked_socket, &blocked).await?;
    let accepted = receive_public_response(&mut blocked_socket).await?;
    let blocked_acceptance = match accepted.response {
        Some(invocation_response::Response::Accepted(accepted)) => accepted,
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

    resume_public_input_with_end(deps, &user.token, blocked_acceptance, blocked_stream_id).await?;

    let subsequent = run_public_session(
        deps,
        &user.token,
        public_start(
            application_name,
            environment_name,
            &agent_name,
            "ping",
            proto_record(Vec::new()),
        ),
    )
    .await?;
    assert!(subsequent.iter().any(|response| matches!(
        response.response,
        Some(invocation_response::Response::Finished(_))
    )));

    let capability_stream_id = 75;
    let mut capability_socket = connect_public_invocation_socket(deps, Some(&user.token)).await?;
    send_public_request(
        &mut capability_socket,
        &public_start(
            application_name,
            environment_name,
            &agent_name,
            "consume",
            ProtoSchemaValue {
                value: Some(schema_value::Value::RecordValue(RecordValue {
                    fields: vec![ProtoSchemaValue {
                        value: Some(schema_value::Value::StreamReference(
                            SchemaValueStreamReference {
                                stream_id: capability_stream_id,
                            },
                        )),
                    }],
                })),
            },
        ),
    )
    .await?;
    let accepted = receive_public_response(&mut capability_socket).await?;
    let capability_acceptance = match accepted.response {
        Some(invocation_response::Response::Accepted(accepted)) => accepted,
        other => anyhow::bail!("capability invocation was not accepted: {other:?}"),
    };
    let capability_durable_stream_id = capability_acceptance
        .stream_mappings
        .iter()
        .find(|mapping| mapping.transport_stream_id == capability_stream_id)
        .and_then(|mapping| mapping.handle.as_ref())
        .and_then(|handle| handle.stream_id)
        .ok_or_else(|| anyhow::anyhow!("capability input mapping omitted its durable stream ID"))?;
    let capability_epoch = capability_acceptance.epoch;
    send_public_request(
        &mut capability_socket,
        &PublicInvocationRequest {
            request: Some(public_invocation_request::Request::InputItem(
                InputStreamItem {
                    transport_stream_id: capability_stream_id,
                    sequence: 0,
                    payload: Some(input_stream_item::Payload::Value(ProtoSchemaValue {
                        value: Some(schema_value::Value::RecordValue(RecordValue {
                            fields: vec![ProtoSchemaValue {
                                value: Some(schema_value::Value::SecretValue(
                                    SecretValue::default(),
                                )),
                            }],
                        })),
                    })),
                    durable_stream_id: Some(capability_durable_stream_id),
                    epoch: capability_epoch,
                },
            )),
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
        capability_acceptance,
        capability_stream_id,
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
                proto_record(Vec::new()),
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
    for invalid_message in [
        Message::Text("not protobuf".into()),
        Message::Binary(vec![0xff, 0xff].into()),
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
            proto_record(Vec::new()),
        ),
    )
    .await?;
    assert_eq!(rejected.len(), 1);
    let Some(invocation_response::Response::Rejected(rejected)) = &rejected[0].response else {
        anyhow::bail!("unresolved public selector did not produce invocation-rejected")
    };
    assert_eq!(
        rejected.reason(),
        InvocationRejectionReason::NotFound,
        "unexpected public rejection: {}",
        rejected.error
    );
    Ok(())
}
