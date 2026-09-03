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
    RecordValue, SchemaValue as ProtoSchemaValue, SchemaValueStreamReference, schema_value,
};
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::{
    DurableStreamMapping, InputStreamEnd, InputStreamItem, InvocationAccepted, InvocationFailure,
    InvocationFailureKind, InvocationRejectionReason, InvocationRequest, InvocationResponse,
    InvocationStart, ResumeAttach as PrivateResumeAttach,
    ResumeOperation as PrivateResumeOperation, StreamCancel, StreamCancelReason, StreamCancelRole,
    input_stream_item, invocation_request, invocation_response, invocation_session_completion,
    invocation_session_result,
};
use golem_client::model::ComponentDto;
use golem_common::base_model::durable_stream::AttachmentId;
use golem_common::model::agent::{AgentMode, ParsedAgentId};
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
use golem_common::schema::{ResultValuePayload, SchemaValue, TypedSchemaValue};
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

use std::collections::{HashMap, HashSet};

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
    caller_type: &str,
    target_type: &str,
    name_prefix: &str,
) -> anyhow::Result<String> {
    for index in 0..10_000 {
        let name = format!("{name_prefix}-{index}");
        let caller = agent_id!(caller_type, name.clone());
        let target = agent_id!(target_type, name.clone());
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

    anyhow::bail!(
        "could not find {caller_type} and {target_type} agent IDs assigned to different executors"
    )
}

async fn invoke_agent_session(
    deps: &EnvBasedTestDependencies,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    method_name: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<Result<SchemaValue, InvocationFailure>> {
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
                anyhow::bail!("invocation session was rejected: {}", rejected.error);
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
                    Some(invocation_session_completion::Outcome::Failure(failure)) => Err(failure),
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

struct TrustedInvocationSession {
    requests: mpsc::Sender<InvocationRequest>,
    responses: mpsc::Receiver<Result<InvocationResponse, String>>,
    response_task: Option<tokio::task::JoinHandle<()>>,
    state: InvocationSessionState,
    acceptance: InvocationAccepted,
    mappings: HashMap<u64, DurableStreamMapping>,
}

impl TrustedInvocationSession {
    async fn start(
        deps: &EnvBasedTestDependencies,
        component: &ComponentDto,
        agent_id: &ParsedAgentId,
        method_name: &str,
        input: ProtoSchemaValue,
    ) -> anyhow::Result<Self> {
        let idempotency_key = IdempotencyKey::fresh();
        let agent_id = agent_id
            .with_ephemeral_invocation_phantom(&idempotency_key)
            .map_err(|error| anyhow::anyhow!("invalid ephemeral agent id: {error}"))?;
        let agent_id = AgentId::from_agent_id(component.id, &agent_id)
            .map_err(|error| anyhow::anyhow!("invalid agent id: {error}"))?;
        let (requests, receiver) = mpsc::channel(32);
        let request = InvocationRequest {
            request: Some(invocation_request::Request::Start(InvocationStart {
                agent_id: Some(agent_id.into()),
                method_name: Some(method_name.to_string()),
                input: Some(input),
                idempotency_key: Some(idempotency_key.into()),
                context: None,
                auth_ctx: Some(AuthCtx::System.into()),
                principal: None,
                environment_id: None,
                config: Vec::new(),
                component_owner_account_id: None,
                mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                schedule_at: None,
                freshness_disposition:
                    golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh
                        as i32,
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                expected_callee_fingerprint: None,
                durable_input_mappings: Vec::new(),
                scope_card: None,
            })),
        };
        let mut state = InvocationSessionState::default();
        state
            .validate_trusted_request(&request)
            .map_err(anyhow::Error::msg)?;
        requests.send(request).await?;

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
        let (response_sender, responses) = mpsc::channel(32);
        let response_task = tokio::spawn(async move {
            loop {
                match inbound.message().await {
                    Ok(Some(response)) => {
                        if response_sender.send(Ok(response)).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => return,
                    Err(error) => {
                        let _ = response_sender.send(Err(error.to_string())).await;
                        return;
                    }
                }
            }
        });
        let mut session = Self {
            requests,
            responses,
            response_task: Some(response_task),
            state,
            acceptance: InvocationAccepted::default(),
            mappings: HashMap::new(),
        };
        let response = session.receive().await?;
        let accepted = match response.response {
            Some(invocation_response::Response::Accepted(accepted)) => accepted,
            Some(invocation_response::Response::Rejected(rejected)) => {
                anyhow::bail!("trusted invocation was rejected: {}", rejected.error)
            }
            other => anyhow::bail!("trusted invocation was not accepted: {other:?}"),
        };
        session.acceptance = accepted;
        Ok(session)
    }

    async fn receive(&mut self) -> anyhow::Result<InvocationResponse> {
        let response =
            tokio::time::timeout(std::time::Duration::from_secs(30), self.responses.recv())
                .await
                .map_err(|_| {
                    anyhow::anyhow!("trusted invocation session made no progress for 30 seconds")
                })?
                .ok_or_else(|| {
                    anyhow::anyhow!("trusted invocation response ended before protocol completion")
                })?
                .map_err(anyhow::Error::msg)?;
        self.state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        self.remember_mappings(&response);
        Ok(response)
    }

    fn remember_mappings(&mut self, response: &InvocationResponse) {
        let mappings = match response.response.as_ref() {
            Some(invocation_response::Response::Accepted(accepted)) => &accepted.stream_mappings,
            Some(invocation_response::Response::Result(result)) => &result.new_stream_mappings,
            Some(invocation_response::Response::OutputItem(item)) => &item.new_stream_mappings,
            Some(invocation_response::Response::InputAck(ack)) => &ack.new_stream_mappings,
            _ => return,
        };
        self.mappings.extend(
            mappings
                .iter()
                .cloned()
                .map(|mapping| (mapping.transport_stream_id, mapping)),
        );
    }

    fn stream_identity(
        &self,
        transport_stream_id: u64,
    ) -> anyhow::Result<golem_api_grpc::proto::golem::common::Uuid> {
        self.mappings
            .get(&transport_stream_id)
            .and_then(|mapping| mapping.handle.as_ref())
            .and_then(|handle| handle.stream_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "stream {transport_stream_id} has no durable invocation-session mapping"
                )
            })
    }

    async fn send(&mut self, request: InvocationRequest) -> anyhow::Result<()> {
        self.state
            .validate_trusted_request(&request)
            .map_err(anyhow::Error::msg)?;
        self.requests.send(request).await?;
        Ok(())
    }

    async fn send_input_value(
        &mut self,
        transport_stream_id: u64,
        sequence: u64,
        value: SchemaValue,
    ) -> anyhow::Result<()> {
        let durable_stream_id = self.stream_identity(transport_stream_id)?;
        self.send(InvocationRequest {
            request: Some(invocation_request::Request::InputItem(InputStreamItem {
                transport_stream_id,
                sequence,
                payload: Some(input_stream_item::Payload::Value(
                    value.try_into().map_err(anyhow::Error::msg)?,
                )),
                durable_stream_id: Some(durable_stream_id),
                epoch: self.acceptance.epoch,
            })),
        })
        .await
    }

    async fn end_input(&mut self, transport_stream_id: u64, sequence: u64) -> anyhow::Result<()> {
        let durable_stream_id = self.stream_identity(transport_stream_id)?;
        self.send(InvocationRequest {
            request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
                transport_stream_id,
                sequence,
                durable_stream_id: Some(durable_stream_id),
                epoch: self.acceptance.epoch,
            })),
        })
        .await
    }

    async fn cancel_stream(
        &mut self,
        transport_stream_id: u64,
        producer_sequence: u64,
        role: StreamCancelRole,
    ) -> anyhow::Result<()> {
        let durable_stream_id = self.stream_identity(transport_stream_id)?;
        self.send(InvocationRequest {
            request: Some(invocation_request::Request::StreamCancel(StreamCancel {
                transport_stream_id,
                producer_sequence,
                role: role as i32,
                reason: StreamCancelReason::Cancelled as i32,
                details: Some("integration test cancelled the stream".to_string()),
                durable_stream_id: Some(durable_stream_id),
                epoch: self.acceptance.epoch,
                durable_offset: Vec::new(),
            })),
        })
        .await
    }

    async fn finish(
        mut self,
        mut report: TrustedInvocationReport,
    ) -> anyhow::Result<TrustedInvocationReport> {
        while !self.state.is_complete() {
            report.record(self.receive().await?)?;
        }
        let trailing =
            tokio::time::timeout(std::time::Duration::from_secs(10), self.responses.recv())
                .await
                .map_err(|_| {
                    anyhow::anyhow!("completed trusted invocation did not close its endpoint")
                })?;
        if let Some(trailing) = trailing {
            anyhow::bail!("trusted invocation emitted a frame after completion: {trailing:?}");
        }
        self.response_task
            .take()
            .expect("trusted invocation response task is present")
            .await?;
        Ok(report)
    }
}

#[derive(Default)]
struct TrustedInvocationReport {
    result: Option<ProtoSchemaValue>,
    outputs: HashMap<u64, Vec<ProtoSchemaValue>>,
    output_ends: HashSet<u64>,
    output_errors: HashMap<u64, String>,
    input_acks: HashMap<u64, usize>,
    stream_cancels: Vec<StreamCancel>,
    completion: Option<Result<(), InvocationFailure>>,
}

impl TrustedInvocationReport {
    fn record(&mut self, response: InvocationResponse) -> anyhow::Result<()> {
        match response.response {
            Some(invocation_response::Response::Result(result)) => {
                if self.result.is_some() {
                    anyhow::bail!("trusted invocation returned more than one result");
                }
                self.result = match result.result {
                    Some(invocation_session_result::Result::MethodResult(value)) => Some(value),
                    Some(invocation_session_result::Result::NoResult(_)) | None => {
                        anyhow::bail!("trusted invocation returned no method result")
                    }
                };
            }
            Some(invocation_response::Response::OutputItem(item)) => {
                self.outputs
                    .entry(item.transport_stream_id)
                    .or_default()
                    .push(
                        item.value.ok_or_else(|| {
                            anyhow::anyhow!("output stream item omitted its value")
                        })?,
                    );
            }
            Some(invocation_response::Response::OutputEnd(end)) => {
                self.output_ends.insert(end.transport_stream_id);
            }
            Some(invocation_response::Response::OutputError(error)) => {
                self.output_errors
                    .insert(error.transport_stream_id, error.details);
            }
            Some(invocation_response::Response::InputAck(ack)) => {
                *self.input_acks.entry(ack.transport_stream_id).or_default() += 1;
            }
            Some(invocation_response::Response::StreamCancel(cancel)) => {
                self.stream_cancels.push(cancel);
            }
            Some(invocation_response::Response::Finished(finished)) => {
                self.completion = Some(match finished.outcome {
                    Some(invocation_session_completion::Outcome::Success(_)) => Ok(()),
                    Some(invocation_session_completion::Outcome::Failure(failure)) => Err(failure),
                    None => anyhow::bail!("trusted invocation completion has no outcome"),
                });
            }
            Some(invocation_response::Response::Rejected(rejected)) => {
                anyhow::bail!("trusted invocation was rejected: {}", rejected.error)
            }
            Some(invocation_response::Response::Accepted(_)) => {
                anyhow::bail!("trusted invocation was accepted more than once")
            }
            Some(invocation_response::Response::AttachmentRevoked(revoked)) => {
                anyhow::bail!(
                    "trusted invocation attachment was revoked: {}",
                    revoked.details
                )
            }
            None => anyhow::bail!("trusted invocation returned an empty response frame"),
        }
        Ok(())
    }

    fn successful_result(&self) -> anyhow::Result<&ProtoSchemaValue> {
        match self.completion.as_ref() {
            Some(Ok(())) => {}
            other => anyhow::bail!("trusted invocation did not complete successfully: {other:?}"),
        }
        self.result
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("trusted invocation completed without a result"))
    }

    fn failure(&self) -> anyhow::Result<&InvocationFailure> {
        match self.completion.as_ref() {
            Some(Err(failure)) => Ok(failure),
            other => anyhow::bail!("trusted invocation did not fail as expected: {other:?}"),
        }
    }

    fn decoded_output(&self, stream_id: u64) -> anyhow::Result<Vec<SchemaValue>> {
        self.outputs
            .get(&stream_id)
            .into_iter()
            .flatten()
            .cloned()
            .map(|value| value.try_into().map_err(anyhow::Error::msg))
            .collect()
    }
}

fn proto_record_values(fields: Vec<ProtoSchemaValue>) -> ProtoSchemaValue {
    ProtoSchemaValue {
        value: Some(schema_value::Value::RecordValue(RecordValue { fields })),
    }
}

fn proto_stream(stream_id: u64) -> ProtoSchemaValue {
    ProtoSchemaValue {
        value: Some(schema_value::Value::StreamReference(
            SchemaValueStreamReference { stream_id },
        )),
    }
}

fn proto_stream_id(value: &ProtoSchemaValue) -> anyhow::Result<u64> {
    match value.value.as_ref() {
        Some(schema_value::Value::StreamReference(stream)) => Ok(stream.stream_id),
        other => anyhow::bail!("expected a stream reference, got {other:?}"),
    }
}

fn proto_record_fields(value: &ProtoSchemaValue) -> anyhow::Result<&[ProtoSchemaValue]> {
    match value.value.as_ref() {
        Some(schema_value::Value::RecordValue(record)) => Ok(&record.fields),
        other => anyhow::bail!("expected a record value, got {other:?}"),
    }
}

fn proto_tuple_elements(value: &ProtoSchemaValue) -> anyhow::Result<&[ProtoSchemaValue]> {
    match value.value.as_ref() {
        Some(schema_value::Value::TupleValue(tuple)) => Ok(&tuple.elements),
        other => anyhow::bail!("expected a tuple value, got {other:?}"),
    }
}

fn decode_proto_value(value: &ProtoSchemaValue) -> anyhow::Result<SchemaValue> {
    value.clone().try_into().map_err(anyhow::Error::msg)
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

fn schema_u32_list(values: impl IntoIterator<Item = u32>) -> SchemaValue {
    SchemaValue::List {
        elements: values.into_iter().map(SchemaValue::U32).collect(),
    }
}

fn schema_string_list(values: impl IntoIterator<Item = impl Into<String>>) -> SchemaValue {
    SchemaValue::List {
        elements: values
            .into_iter()
            .map(|value| SchemaValue::String(value.into()))
            .collect(),
    }
}

fn assert_moonbit_streaming_report(value: SchemaValue, target_name: &str) {
    assert_eq!(
        value,
        SchemaValue::Record {
            fields: vec![
                schema_u32_list([1, 2, 3]),
                schema_u32_list([3, 5, 8]),
                schema_u32_list([104, 105, 106]),
                SchemaValue::String("moonbit".to_string()),
                schema_u32_list([13, 21, 34]),
                schema_string_list(["alpha", "beta", "gamma"]),
                schema_u32_list([7, 8, 9]),
                schema_string_list(["item-0", "item-1", "item-2"]),
                SchemaValue::List {
                    elements: (0..3)
                        .map(|index| schema_u32_list(index * 10..index * 10 + 4))
                        .collect(),
                },
                schema_u32_list(0..192),
                schema_u32_list(1000..1192),
                schema_u32_list(2000..2192),
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::Result(ResultValuePayload::Ok {
                            value: Some(Box::new(SchemaValue::U32(1))),
                        }),
                        SchemaValue::Result(ResultValuePayload::Err {
                            value: Some(Box::new(SchemaValue::String("recoverable".to_string(),))),
                        }),
                        SchemaValue::Result(ResultValuePayload::Ok {
                            value: Some(Box::new(SchemaValue::U32(2))),
                        }),
                    ],
                },
                SchemaValue::U32(0),
                SchemaValue::String(format!("pong:{target_name}")),
                SchemaValue::U64(1),
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
    let name = cross_executor_agent_name(
        &component,
        &routing_table,
        "StreamingRpcCaller",
        "StreamingRpcTarget",
        "generated-rust-streaming-rpc-cross",
    )?;
    let caller_agent_id = agent_id!("StreamingRpcCaller", name);

    let result = invoke_agent_session(deps, &component, &caller_agent_id, "run", data_value!())
        .await?
        .map_err(|failure| anyhow::Error::msg(failure.message))?;
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
    assert_eq!(producer_error.kind(), InvocationFailureKind::Execution);
    assert!(
        producer_error.message.contains("Component trapped")
            || producer_error
                .message
                .contains("value-node index out of range: 0"),
        "unexpected producer error: {producer_error:?}"
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
#[timeout("8 minutes")]
#[tracing::instrument]
async fn generated_moonbit_client_streaming_rpc_cross_executor(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, environment) = user.app_and_env().await?;
    let component = user
        .component(&environment.id, "golem_it_agent_rpc_moonbit_release")
        .name("golem-it:agent-rpc-moonbit")
        .unique()
        .store()
        .await?;
    let routing_table = deps.shard_manager().get_routing_table().await?;
    let target_name = cross_executor_agent_name(
        &component,
        &routing_table,
        "MoonbitStreamingRpcCaller",
        "MoonbitStreamingRpcCrossTarget",
        "generated-moonbit-streaming-rpc-cross",
    )?;
    let caller_agent_id = agent_id!("MoonbitStreamingRpcCaller", target_name.clone());

    let report = invoke_agent_session(deps, &component, &caller_agent_id, "run", data_value!())
        .await?
        .map_err(|failure| anyhow::Error::msg(failure.message))?;
    assert_moonbit_streaming_report(report, &target_name);

    let after_reader_drop = invoke_agent_session(
        deps,
        &component,
        &caller_agent_id,
        "call_scalar",
        data_value!(),
    )
    .await?
    .map_err(|failure| anyhow::Error::msg(failure.message))?;
    assert_eq!(after_reader_drop, SchemaValue::U64(2));

    let failure_name = cross_executor_agent_name(
        &component,
        &routing_table,
        "MoonbitStreamingRpcCaller",
        "MoonbitStreamingRpcCrossTarget",
        "generated-moonbit-producer-failure-cross",
    )?;
    let failure_caller = agent_id!("MoonbitStreamingRpcCaller", failure_name);
    let failure = invoke_agent_session(
        deps,
        &component,
        &failure_caller,
        "call_producer_failure",
        data_value!(),
    )
    .await?
    .expect_err("remote MoonBit producer failure must fail the caller invocation");
    assert_eq!(failure.kind(), InvocationFailureKind::Execution);
    assert!(
        !failure.message.is_empty(),
        "remote producer failure must retain an execution diagnostic"
    );

    let control_name = cross_executor_agent_name(
        &component,
        &routing_table,
        "MoonbitStreamingRpcCaller",
        "MoonbitStreamingRpcCrossTarget",
        "generated-moonbit-after-failure-cross",
    )?;
    let control_caller = agent_id!("MoonbitStreamingRpcCaller", control_name);
    for expected in [1, 2] {
        let value = invoke_agent_session(
            deps,
            &component,
            &control_caller,
            "call_scalar",
            data_value!(),
        )
        .await?
        .map_err(|failure| anyhow::Error::msg(failure.message))?;
        assert_eq!(value, SchemaValue::U64(expected));
    }

    let cancellation_name = cross_executor_agent_name(
        &component,
        &routing_table,
        "MoonbitStreamingRpcCaller",
        "MoonbitStreamingRpcCrossTarget",
        "generated-moonbit-cancellation-cross",
    )?;
    let cancellation_caller = agent_id!("MoonbitStreamingRpcCaller", cancellation_name.clone());
    let cancellation_target =
        agent_id!("MoonbitStreamingRpcCrossTarget", cancellation_name.clone());
    let producer_cancellation = invoke_agent_session(
        deps,
        &component,
        &cancellation_caller,
        "call_producer_cancellation",
        data_value!(),
    )
    .await?
    .map_err(|failure| anyhow::Error::msg(failure.message))?;
    assert_eq!(
        producer_cancellation,
        SchemaValue::Tuple {
            elements: vec![
                SchemaValue::U32(43),
                SchemaValue::String(format!("pong:{cancellation_name}")),
            ],
        }
    );
    let target_after_cancellation = user
        .invoke_and_await_agent(&component, &cancellation_target, "increment", data_value!())
        .await?
        .into_typed::<u64>()?;
    assert_eq!(target_after_cancellation, 1);

    let server_cancellation_name = cross_executor_agent_name(
        &component,
        &routing_table,
        "MoonbitStreamingRpcCaller",
        "MoonbitStreamingRpcCrossTarget",
        "generated-moonbit-server-cancellation-cross",
    )?;
    let server_cancellation_caller = agent_id!(
        "MoonbitStreamingRpcCaller",
        server_cancellation_name.clone()
    );
    let server_cancellation = invoke_agent_session(
        deps,
        &component,
        &server_cancellation_caller,
        "call_server_cancellation",
        data_value!(),
    )
    .await?
    .map_err(|failure| anyhow::Error::msg(failure.message))?;
    assert_eq!(
        server_cancellation,
        SchemaValue::Tuple {
            elements: vec![
                SchemaValue::U32(50),
                SchemaValue::Bool(true),
                SchemaValue::String(format!("pong:{server_cancellation_name}")),
            ],
        }
    );
    let generated_after_server_cancellation = invoke_agent_session(
        deps,
        &component,
        &server_cancellation_caller,
        "call_scalar",
        data_value!(),
    )
    .await?
    .map_err(|failure| anyhow::Error::msg(failure.message))?;
    assert_eq!(generated_after_server_cancellation, SchemaValue::U64(1));

    Ok(())
}

#[test]
#[timeout("8 minutes")]
#[tracing::instrument]
async fn moonbit_direct_guest_abi_streaming_lifecycle(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, environment) = user.app_and_env().await?;
    let component = user
        .component(&environment.id, "golem_it_agent_rpc_moonbit_release")
        .name("golem-it:agent-rpc-moonbit")
        .unique()
        .store()
        .await?;
    let name = format!("moonbit-direct-streaming-{}", uuid::Uuid::new_v4());
    let target_agent_id = agent_id!("MoonbitStreamingRpcTarget", name.clone());
    assert_eq!(
        component
            .metadata
            .find_agent_type_by_name_ref(&target_agent_id.agent_type)
            .ok_or_else(|| anyhow::anyhow!("MoonBit streaming target metadata is missing"))?
            .mode,
        AgentMode::Ephemeral
    );

    let consume_stream_id = 101;
    let mut consume = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "consume",
        proto_record_values(vec![proto_stream(consume_stream_id)]),
    )
    .await?;
    for (sequence, value) in [1_u32, 2, 3].into_iter().enumerate() {
        consume
            .send_input_value(consume_stream_id, sequence as u64, SchemaValue::U32(value))
            .await?;
    }
    consume.end_input(consume_stream_id, 3).await?;
    let consume = consume.finish(TrustedInvocationReport::default()).await?;
    assert_eq!(
        decode_proto_value(consume.successful_result()?)?,
        SchemaValue::List {
            elements: vec![
                SchemaValue::U32(1),
                SchemaValue::U32(2),
                SchemaValue::U32(3),
            ],
        }
    );
    assert_eq!(consume.input_acks.get(&consume_stream_id), Some(&4));

    let produce = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "produce",
        proto_record_values(Vec::new()),
    )
    .await?
    .finish(TrustedInvocationReport::default())
    .await?;
    let produce_stream_id = proto_stream_id(produce.successful_result()?)?;
    assert_eq!(
        produce.decoded_output(produce_stream_id)?,
        vec![
            SchemaValue::U32(3),
            SchemaValue::U32(5),
            SchemaValue::U32(8),
        ]
    );
    assert!(produce.output_ends.contains(&produce_stream_id));
    assert!(produce.output_errors.is_empty());

    let transform_input_id = 102;
    let mut transform = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "transform",
        proto_record_values(vec![proto_stream(transform_input_id)]),
    )
    .await?;
    for (sequence, value) in [4_u32, 5, 6].into_iter().enumerate() {
        transform
            .send_input_value(transform_input_id, sequence as u64, SchemaValue::U32(value))
            .await?;
    }
    transform.end_input(transform_input_id, 3).await?;
    let transform = transform.finish(TrustedInvocationReport::default()).await?;
    let transform_output_id = proto_stream_id(transform.successful_result()?)?;
    assert_eq!(
        transform.decoded_output(transform_output_id)?,
        vec![
            SchemaValue::U32(104),
            SchemaValue::U32(105),
            SchemaValue::U32(106),
        ]
    );
    assert_eq!(transform.input_acks.get(&transform_input_id), Some(&4));
    assert!(transform.output_ends.contains(&transform_output_id));

    let scalar_and_stream = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "scalar_and_stream",
        proto_record_values(Vec::new()),
    )
    .await?
    .finish(TrustedInvocationReport::default())
    .await?;
    let scalar_and_stream_result = proto_tuple_elements(scalar_and_stream.successful_result()?)?;
    assert_eq!(scalar_and_stream_result.len(), 2);
    assert_eq!(
        decode_proto_value(&scalar_and_stream_result[0])?,
        SchemaValue::String("moonbit".to_string())
    );
    let scalar_stream_id = proto_stream_id(&scalar_and_stream_result[1])?;
    assert_eq!(
        scalar_and_stream.decoded_output(scalar_stream_id)?,
        vec![
            SchemaValue::U32(13),
            SchemaValue::U32(21),
            SchemaValue::U32(34),
        ]
    );
    assert!(scalar_and_stream.output_ends.contains(&scalar_stream_id));

    let nested_labels_id = 103;
    let nested_values_id = 104;
    let mut nested_input = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "consume_nested",
        proto_record_values(vec![proto_record_values(vec![
            proto_stream(nested_labels_id),
            proto_stream(nested_values_id),
        ])]),
    )
    .await?;
    for (sequence, label) in ["alpha", "beta", "gamma"].into_iter().enumerate() {
        nested_input
            .send_input_value(
                nested_labels_id,
                sequence as u64,
                SchemaValue::String(label.to_string()),
            )
            .await?;
    }
    nested_input.end_input(nested_labels_id, 3).await?;
    for (sequence, value) in [7_u32, 8, 9].into_iter().enumerate() {
        nested_input
            .send_input_value(nested_values_id, sequence as u64, SchemaValue::U32(value))
            .await?;
    }
    nested_input.end_input(nested_values_id, 3).await?;
    let nested_input = nested_input
        .finish(TrustedInvocationReport::default())
        .await?;
    let nested_input_result = proto_tuple_elements(nested_input.successful_result()?)?;
    assert_eq!(nested_input_result.len(), 2);
    assert_eq!(
        decode_proto_value(&nested_input_result[0])?,
        SchemaValue::List {
            elements: ["alpha", "beta", "gamma"]
                .into_iter()
                .map(|value| SchemaValue::String(value.to_string()))
                .collect(),
        }
    );
    assert_eq!(
        decode_proto_value(&nested_input_result[1])?,
        SchemaValue::List {
            elements: vec![
                SchemaValue::U32(7),
                SchemaValue::U32(8),
                SchemaValue::U32(9),
            ],
        }
    );
    assert_eq!(nested_input.input_acks.get(&nested_labels_id), Some(&4));
    assert_eq!(nested_input.input_acks.get(&nested_values_id), Some(&4));

    let nested_output = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "produce_nested_items",
        proto_record_values(Vec::new()),
    )
    .await?
    .finish(TrustedInvocationReport::default())
    .await?;
    let nested_outer_id = proto_stream_id(nested_output.successful_result()?)?;
    let nested_items = nested_output
        .outputs
        .get(&nested_outer_id)
        .ok_or_else(|| anyhow::anyhow!("nested output stream returned no items"))?;
    assert_eq!(nested_items.len(), 3);
    for (index, item) in nested_items.iter().enumerate() {
        let fields = proto_record_fields(item)?;
        assert_eq!(fields.len(), 2);
        assert_eq!(
            decode_proto_value(&fields[0])?,
            SchemaValue::String(format!("item-{index}"))
        );
        let child_stream_id = proto_stream_id(&fields[1])?;
        assert_eq!(
            nested_output.decoded_output(child_stream_id)?,
            (0..4)
                .map(|offset| SchemaValue::U32((index * 10 + offset) as u32))
                .collect::<Vec<_>>()
        );
        assert!(nested_output.output_ends.contains(&child_stream_id));
    }
    assert!(nested_output.output_ends.contains(&nested_outer_id));
    assert!(nested_output.output_errors.is_empty());

    let siblings = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "produce_siblings",
        proto_record_values(Vec::new()),
    )
    .await?
    .finish(TrustedInvocationReport::default())
    .await?;
    let sibling_result = proto_tuple_elements(siblings.successful_result()?)?;
    assert_eq!(sibling_result.len(), 2);
    let first_sibling_id = proto_stream_id(&sibling_result[0])?;
    let second_sibling_id = proto_stream_id(&sibling_result[1])?;
    assert_eq!(
        siblings.decoded_output(first_sibling_id)?,
        (0..192).map(SchemaValue::U32).collect::<Vec<_>>()
    );
    assert_eq!(
        siblings.decoded_output(second_sibling_id)?,
        (1000..1192).map(SchemaValue::U32).collect::<Vec<_>>()
    );
    assert!(siblings.output_ends.contains(&first_sibling_id));
    assert!(siblings.output_ends.contains(&second_sibling_id));

    let forward_input_id = 105;
    let mut forward = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "forward_unread",
        proto_record_values(vec![proto_stream(forward_input_id)]),
    )
    .await?;
    for (sequence, value) in [200_u32, 201, 202].into_iter().enumerate() {
        forward
            .send_input_value(forward_input_id, sequence as u64, SchemaValue::U32(value))
            .await?;
    }
    forward.end_input(forward_input_id, 3).await?;
    let forward = forward.finish(TrustedInvocationReport::default()).await?;
    let forward_output_id = proto_stream_id(forward.successful_result()?)?;
    assert_eq!(
        forward.decoded_output(forward_output_id)?,
        vec![
            SchemaValue::U32(200),
            SchemaValue::U32(201),
            SchemaValue::U32(202),
        ]
    );
    assert_eq!(forward.input_acks.get(&forward_input_id), Some(&4));
    assert!(forward.output_ends.contains(&forward_output_id));

    let recoverable = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "produce_recoverable_results",
        proto_record_values(Vec::new()),
    )
    .await?
    .finish(TrustedInvocationReport::default())
    .await?;
    let recoverable_stream_id = proto_stream_id(recoverable.successful_result()?)?;
    assert_eq!(
        recoverable.decoded_output(recoverable_stream_id)?,
        vec![
            SchemaValue::Result(ResultValuePayload::Ok {
                value: Some(Box::new(SchemaValue::U32(1))),
            }),
            SchemaValue::Result(ResultValuePayload::Err {
                value: Some(Box::new(SchemaValue::String("recoverable".to_string()))),
            }),
            SchemaValue::Result(ResultValuePayload::Ok {
                value: Some(Box::new(SchemaValue::U32(2))),
            }),
        ]
    );
    assert!(recoverable.output_ends.contains(&recoverable_stream_id));

    let producer_failure = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "produce_failure",
        proto_record_values(Vec::new()),
    )
    .await?
    .finish(TrustedInvocationReport::default())
    .await?;
    assert!(producer_failure.result.is_none());
    assert!(producer_failure.outputs.is_empty());
    assert!(producer_failure.output_ends.is_empty());
    assert!(producer_failure.output_errors.is_empty());
    assert_eq!(
        producer_failure.failure()?.kind(),
        InvocationFailureKind::Execution
    );
    assert!(
        !producer_failure.failure()?.message.is_empty(),
        "producer failure must retain an execution diagnostic"
    );
    assert_moonbit_target_ping(deps, &component, &target_agent_id, &name).await?;

    let cancellable = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "produce_cancellable",
        proto_record_values(Vec::new()),
    )
    .await?;
    let mut cancellable = cancellable;
    let mut cancellable_report = TrustedInvocationReport::default();
    let (cancellable_stream_id, cancel_sequence) = loop {
        let response = cancellable.receive().await?;
        let observed = match response.response.as_ref() {
            Some(invocation_response::Response::OutputItem(item)) => {
                Some((item.transport_stream_id, item.producer_sequence + 1))
            }
            _ => None,
        };
        cancellable_report.record(response)?;
        if let Some(observed) = observed {
            break observed;
        }
    };
    assert_eq!(
        cancellable_report.decoded_output(cancellable_stream_id)?,
        vec![SchemaValue::U32(43)]
    );
    cancellable
        .cancel_stream(
            cancellable_stream_id,
            cancel_sequence,
            StreamCancelRole::OutputConsumer,
        )
        .await?;
    let cancellable = cancellable.finish(cancellable_report).await?;
    let result_stream_id = proto_stream_id(cancellable.successful_result()?)?;
    assert_eq!(result_stream_id, cancellable_stream_id);
    assert!(!cancellable.output_ends.contains(&cancellable_stream_id));
    assert!(cancellable.stream_cancels.iter().any(|cancel| {
        cancel.transport_stream_id == cancellable_stream_id
            && cancel.role() == StreamCancelRole::OutputProducer
            && cancel.reason() == StreamCancelReason::Cancelled
    }));
    assert_moonbit_target_ping(deps, &component, &target_agent_id, &name).await?;

    let cancelled_input_id = 106;
    let mut cancelled_input = TrustedInvocationSession::start(
        deps,
        &component,
        &target_agent_id,
        "consume",
        proto_record_values(vec![proto_stream(cancelled_input_id)]),
    )
    .await?;
    cancelled_input
        .send_input_value(cancelled_input_id, 0, SchemaValue::U32(99))
        .await?;
    let mut cancelled_input_report = TrustedInvocationReport::default();
    while cancelled_input_report
        .input_acks
        .get(&cancelled_input_id)
        .copied()
        .unwrap_or_default()
        == 0
    {
        let response = cancelled_input.receive().await?;
        cancelled_input_report.record(response)?;
    }
    cancelled_input
        .cancel_stream(cancelled_input_id, 1, StreamCancelRole::InputProducer)
        .await?;
    let cancelled_input = cancelled_input.finish(cancelled_input_report).await?;
    assert_eq!(
        decode_proto_value(cancelled_input.successful_result()?)?,
        SchemaValue::List {
            elements: vec![SchemaValue::U32(99)],
        }
    );
    assert_moonbit_target_ping(deps, &component, &target_agent_id, &name).await?;

    Ok(())
}

async fn assert_moonbit_target_ping(
    deps: &EnvBasedTestDependencies,
    component: &ComponentDto,
    target_agent_id: &ParsedAgentId,
    name: &str,
) -> anyhow::Result<()> {
    let ping = TrustedInvocationSession::start(
        deps,
        component,
        target_agent_id,
        "ping",
        proto_record_values(Vec::new()),
    )
    .await?
    .finish(TrustedInvocationReport::default())
    .await?;
    let ping = decode_proto_value(ping.successful_result()?)?;
    assert_eq!(ping, SchemaValue::String(format!("pong:{name}")));
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
