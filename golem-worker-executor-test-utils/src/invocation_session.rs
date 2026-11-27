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

//! Invocation requests and raw invocation sessions for tests that need the stream frames.
//!
//! [`TestWorkerExecutor::invocation`] builds an [`InvocationStart`] that names only what a test
//! varies. [`TestWorkerExecutor::open_session`] sends it and gives an [`InvocationSession`], which
//! validates every frame in both directions with [`InvocationSessionState`].

use crate::TestWorkerExecutor;
use crate::callers;
use crate::dsl_impl::{invocation_target, proto_agent_key};
use crate::invocation_error::InvocationError;
use anyhow::{anyhow, bail};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::worker::invocation_session_completion::Outcome;
use golem_api_grpc::proto::golem::worker::{
    AgentInvocationMode, AgentTarget, DurableStreamMapping, InputStreamItem, InvocationAccepted,
    InvocationFreshnessDisposition, InvocationRequest, InvocationResponse, InvocationSessionResult,
    InvocationStart, OutputStreamItem, ResumeAttach, ResumeOperation, StreamConsumerCancelled,
    StreamDirection, StreamEnd, StreamTerminal as ProtoStreamTerminal, StreamTerminalAuthor,
    agent_target, input_stream_item, invocation_request, invocation_response,
    invocation_session_result, stream_terminal,
};
use golem_common::model::agent::{OwnedAgentId, ParsedAgentKey};
use golem_common::model::caller::Caller;
use golem_common::model::component::ComponentDto;
use golem_common::model::durable_stream::StreamTerminal;
use golem_common::model::{IdempotencyKey, InvocationRequestKey};
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_schema::proto::golem::schema::{
    RecordValue, SchemaValue as ProtoSchemaValue, SchemaValueStreamReference, schema_value,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Streaming;
use uuid::Uuid;

/// The buffer of the request channel of a session; it holds the frames a test sends before the
/// executor reads them.
const SESSION_REQUEST_BUFFER: usize = 8;

/// Builds an [`InvocationStart`]. [`TestWorkerExecutor::invocation`] starts it with the
/// executor's auth context, an anonymous caller, the component's environment and owner, and a
/// fresh idempotency key. Every other field, including the request key, the component ID and the
/// attempt ID, stays unset until a method sets it, so a converted request sends exactly the
/// fields that it sent before.
#[must_use]
pub struct InvocationStartBuilder<'a> {
    executor: &'a TestWorkerExecutor,
    component: &'a ComponentDto,
    start: InvocationStart,
    idempotency_key: IdempotencyKey,
    key_target: Option<ParsedAgentKey>,
    with_request_key: bool,
}

impl TestWorkerExecutor {
    /// Starts an [`InvocationStartBuilder`] for `method` of an agent of `component`.
    pub fn invocation<'a>(
        &'a self,
        component: &'a ComponentDto,
        method: &str,
    ) -> InvocationStartBuilder<'a> {
        InvocationStartBuilder {
            executor: self,
            component,
            start: InvocationStart {
                method_name: Some(method.to_string()),
                auth_ctx: Some(self.auth_ctx().into()),
                caller: Some(callers::anonymous().into()),
                environment_id: Some(component.environment_id.into()),
                component_owner_account_id: Some(component.account_id.into()),
                ..Default::default()
            },
            idempotency_key: IdempotencyKey::fresh(),
            key_target: None,
            with_request_key: false,
        }
    }

    /// Sends `start` and opens its session. Reads no response. The session validates every
    /// frame in both directions.
    pub async fn open_session(&self, start: InvocationStart) -> anyhow::Result<InvocationSession> {
        self.open_session_with(invocation_request::Request::Start(start), true)
            .await
    }

    /// Sends `resume` and opens its session. Reads no response. The session validates every
    /// frame in both directions.
    pub async fn resume_session(&self, resume: ResumeAttach) -> anyhow::Result<InvocationSession> {
        self.open_session_with(invocation_request::Request::ResumeAttach(resume), true)
            .await
    }

    /// Sends `start` and opens its session without client-side validation: the session sends
    /// and reads frames as they are, for a test of what the executor does with a frame that the
    /// protocol state would refuse.
    pub async fn open_unvalidated_session(
        &self,
        start: InvocationStart,
    ) -> anyhow::Result<InvocationSession> {
        self.open_session_with(invocation_request::Request::Start(start), false)
            .await
    }

    /// As [`Self::open_unvalidated_session`], for a resume attach.
    pub async fn resume_unvalidated_session(
        &self,
        resume: ResumeAttach,
    ) -> anyhow::Result<InvocationSession> {
        self.open_session_with(invocation_request::Request::ResumeAttach(resume), false)
            .await
    }

    async fn open_session_with(
        &self,
        first: invocation_request::Request,
        validate: bool,
    ) -> anyhow::Result<InvocationSession> {
        let (requests, receiver) = mpsc::channel(SESSION_REQUEST_BUFFER);
        let mut state = InvocationSessionState::default();
        let request = InvocationRequest {
            request: Some(first),
        };
        if validate {
            state
                .validate_trusted_request(&request)
                .map_err(anyhow::Error::msg)?;
        }
        requests
            .send(request)
            .await
            .map_err(|_| anyhow!("the invocation session closed before its start"))?;
        let responses = self
            .client
            .clone()
            .invoke_agent_session(ReceiverStream::new(receiver))
            .await?
            .into_inner();
        Ok(InvocationSession {
            requests,
            responses,
            state: validate.then_some(state),
            accepted: None,
        })
    }
}

impl InvocationStartBuilder<'_> {
    /// Targets the existing agent `agent_id`. An ID target never creates an agent.
    pub fn on_agent(mut self, agent_id: &OwnedAgentId) -> Self {
        self.start.target = Some(id_target(agent_id));
        self
    }

    /// Targets the keyed agent of `key`. For an ephemeral agent type it targets the agent that
    /// the idempotency key and the method name derive, and sends `key` as its constructor.
    pub fn on_key(mut self, key: &ParsedAgentKey) -> Self {
        self.key_target = Some(key.clone());
        self
    }

    /// Sets the component ID of the start.
    pub fn with_component_id(mut self) -> Self {
        self.start.component_id = Some(self.component.id.into());
        self
    }

    /// Sets the input, for example `.input(data_value!(17u32))?`.
    pub fn input(mut self, input: TypedSchemaValue) -> anyhow::Result<Self> {
        let (_, value) = input.into_parts();
        self.start.input = Some(value.try_into().map_err(anyhow::Error::msg)?);
        Ok(self)
    }

    /// Sets the input to a record of `fields` in their wire form, for an input that holds a
    /// [`stream_reference`], which has no typed schema value.
    pub fn input_record(mut self, fields: Vec<ProtoSchemaValue>) -> Self {
        self.start.input = Some(ProtoSchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue { fields })),
        });
        self
    }

    /// Sets the idempotency key; [`Self::build`] sends a fresh one otherwise.
    pub fn idempotency_key(mut self, key: &IdempotencyKey) -> Self {
        self.idempotency_key = key.clone();
        self
    }

    /// Sends the external request key of the idempotency key, as the worker service does.
    pub fn with_request_key(mut self) -> Self {
        self.with_request_key = true;
        self
    }

    pub fn allow_streams(mut self) -> Self {
        self.start.allow_streams = true;
        self
    }

    pub fn mode(mut self, mode: AgentInvocationMode) -> Self {
        self.start.mode = mode as i32;
        self
    }

    pub fn freshness(mut self, freshness: InvocationFreshnessDisposition) -> Self {
        self.start.freshness_disposition = freshness as i32;
        self
    }

    pub fn caller(mut self, caller: Caller) -> Self {
        self.start.caller = Some(caller.into());
        self
    }

    /// Sets a fresh attempt ID.
    pub fn fresh_attempt(mut self) -> Self {
        self.start.attempt_id = Some(Uuid::new_v4().into());
        self
    }

    pub fn expect_fingerprint(mut self, fingerprint: Uuid) -> Self {
        self.start.expected_callee_fingerprint = Some(fingerprint.into());
        self
    }

    pub fn build(self) -> anyhow::Result<InvocationStart> {
        let mut start = self.start;
        let request_key = InvocationRequestKey::external(
            self.executor.auth_ctx().idempotency_scope(),
            Some(&self.idempotency_key),
        );
        if let Some(key) = &self.key_target {
            let owner_key = request_key.owner_key(start.method_name.as_deref().unwrap_or_default());
            let (target, constructor) = invocation_target(self.component, key, &owner_key)?;
            start.target = Some(target);
            start.constructor = constructor;
        }
        if self.with_request_key {
            start.request_key = Some(request_key.into());
        }
        start.idempotency_key = Some(self.idempotency_key.into());
        Ok(start)
    }
}

/// A raw invocation session. Unless it was opened unvalidated, every frame it sends or receives
/// passes the validation of [`InvocationSessionState`], as the worker service applies it.
pub struct InvocationSession {
    requests: mpsc::Sender<InvocationRequest>,
    responses: Streaming<InvocationResponse>,
    /// The protocol state; `None` for an unvalidated session.
    state: Option<InvocationSessionState>,
    accepted: Option<InvocationAccepted>,
}

/// How a session ended.
#[derive(Debug)]
pub enum SessionTerminal {
    Succeeded,
    Failed(InvocationError),
    Rejected(InvocationError),
}

impl SessionTerminal {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

/// What [`InvocationSession::collect`] read up to the end of the session.
#[derive(Debug)]
pub struct SessionOutcome {
    pub result: Option<InvocationSessionResult>,
    pub terminal: SessionTerminal,
    pub output_items: Vec<OutputStreamItem>,
    /// The protocol state saw the whole session.
    pub complete: bool,
}

impl InvocationSession {
    /// Reads the acceptance, and the readiness that follows it when the invocation is queued.
    /// Fails on any other frame.
    pub async fn accepted(&mut self) -> anyhow::Result<InvocationAccepted> {
        let accepted = self.acceptance().await?;
        self.ready(accepted).await
    }

    /// Reads only the acceptance, which for a queued invocation has no component revision and
    /// no stream mappings yet. Fails on any other frame.
    pub async fn acceptance(&mut self) -> anyhow::Result<InvocationAccepted> {
        match self.next().await? {
            Some(invocation_response::Response::Accepted(accepted)) => Ok(accepted),
            Some(invocation_response::Response::Rejected(rejected)) => {
                Err(InvocationError::rejected(rejected).into())
            }
            other => bail!("expected the acceptance of the session, got {other:?}"),
        }
    }

    /// The acceptance with which the session sends stream frames: `accepted` itself, or for a
    /// queued invocation, the readiness that the session reads next. Only an awaited method
    /// invocation without a component revision is queued; do not call this for a schedule,
    /// lookup or external-tool session.
    pub async fn ready(
        &mut self,
        accepted: InvocationAccepted,
    ) -> anyhow::Result<InvocationAccepted> {
        let accepted = if accepted.component_revision.is_some() {
            accepted
        } else {
            match self.next().await? {
                Some(invocation_response::Response::Ready(ready)) => ready,
                other => bail!("expected the readiness of a queued session, got {other:?}"),
            }
        };
        self.accepted = Some(accepted.clone());
        Ok(accepted)
    }

    /// The next validated response, or `None` at the end of the session. Fails on a frame
    /// without a response.
    pub async fn next(&mut self) -> anyhow::Result<Option<invocation_response::Response>> {
        match self.message().await? {
            Some(InvocationResponse {
                response: Some(response),
            }) => Ok(Some(response)),
            Some(InvocationResponse { response: None }) => bail!("empty invocation session frame"),
            None => Ok(None),
        }
    }

    /// The next validated response frame as it arrived, or `None` at the end of the session. An
    /// acceptance with a component revision, or a readiness, becomes the acceptance with which
    /// the session sends stream frames.
    pub async fn message(&mut self) -> anyhow::Result<Option<InvocationResponse>> {
        let Some(response) = self.responses.message().await? else {
            return Ok(None);
        };
        if let Some(state) = &mut self.state {
            state
                .validate_response(&response)
                .map_err(anyhow::Error::msg)?;
        }
        match &response.response {
            Some(invocation_response::Response::Accepted(accepted))
                if accepted.component_revision.is_some() =>
            {
                self.accepted = Some(accepted.clone());
            }
            Some(invocation_response::Response::Ready(ready)) => {
                self.accepted = Some(ready.clone());
            }
            _ => {}
        }
        Ok(Some(response))
    }

    /// Reads up to the result. Fails on a rejection or when the session finishes first.
    pub async fn wait_for_result(&mut self) -> anyhow::Result<InvocationSessionResult> {
        loop {
            match self.next().await? {
                Some(invocation_response::Response::Result(result)) => return Ok(result),
                Some(invocation_response::Response::Rejected(rejected)) => {
                    return Err(InvocationError::rejected(rejected).into());
                }
                Some(invocation_response::Response::Finished(finished)) => {
                    bail!("the session finished before its result: {finished:?}")
                }
                Some(_) => {}
                None => bail!("the session ended before its result"),
            }
        }
    }

    /// Sends a validated request, for a frame without its own method.
    pub async fn send(&mut self, request: invocation_request::Request) -> anyhow::Result<()> {
        let request = InvocationRequest {
            request: Some(request),
        };
        if let Some(state) = &mut self.state {
            state
                .validate_trusted_request(&request)
                .map_err(anyhow::Error::msg)?;
        }
        self.requests
            .send(request)
            .await
            .map_err(|_| anyhow!("the invocation session closed its requests"))
    }

    /// Sends a whole request frame, validated unless the session is unvalidated.
    pub async fn send_request(&mut self, request: InvocationRequest) -> anyhow::Result<()> {
        match request.request {
            Some(request) => self.send(request).await,
            None => bail!("an invocation request needs a frame"),
        }
    }

    /// Sends a frame without client-side validation, for a test of a frame that the protocol
    /// refuses, such as a second start on the same session.
    pub async fn send_unvalidated(
        &mut self,
        request: invocation_request::Request,
    ) -> anyhow::Result<()> {
        self.requests
            .send(InvocationRequest {
                request: Some(request),
            })
            .await
            .map_err(|_| anyhow!("the invocation session closed its requests"))
    }

    /// Closes the request stream of the session, as a client that stops sending, and keeps
    /// reading its responses.
    pub fn close_requests(&mut self) {
        let (closed, _) = mpsc::channel(1);
        self.requests = closed;
    }

    /// Sends `value` as item `sequence` of the input stream `stream`, with the durable stream ID
    /// and the epoch of the acceptance.
    pub async fn send_input(
        &mut self,
        stream: u64,
        sequence: u64,
        value: SchemaValue,
    ) -> anyhow::Result<()> {
        let (durable_stream_id, epoch) = self.mapping(stream)?;
        self.send(invocation_request::Request::InputItem(InputStreamItem {
            transport_stream_id: stream,
            sequence,
            payload: Some(input_stream_item::Payload::Value(
                value.try_into().map_err(anyhow::Error::msg)?,
            )),
            durable_stream_id,
            epoch,
        }))
        .await
    }

    /// Ends the input stream `stream` at `sequence` with a guest-authored `Finished`.
    pub async fn finish_input(&mut self, stream: u64, sequence: u64) -> anyhow::Result<()> {
        self.end_input(stream, sequence, StreamTerminal::Finished)
            .await
    }

    /// Ends the input stream `stream` at `sequence` with the guest-authored `terminal`.
    pub async fn end_input(
        &mut self,
        stream: u64,
        sequence: u64,
        terminal: StreamTerminal,
    ) -> anyhow::Result<()> {
        self.end_stream(stream, sequence, StreamDirection::Input, terminal)
            .await
    }

    /// Cancels the output stream `stream` with a guest-authored `ConsumerCancelled`.
    pub async fn cancel_output(
        &mut self,
        stream: u64,
        durable_stream_id: Option<golem_schema::proto::golem::common::Uuid>,
        details: &str,
    ) -> anyhow::Result<()> {
        let epoch = self.accepted_epoch()?;
        self.send(invocation_request::Request::StreamEnd(StreamEnd {
            transport_stream_id: stream,
            producer_sequence: 0,
            direction: StreamDirection::Output as i32,
            authored_by: StreamTerminalAuthor::Guest as i32,
            outcome: Some(ProtoStreamTerminal {
                terminal: Some(stream_terminal::Terminal::ConsumerCancelled(
                    StreamConsumerCancelled {
                        details: Some(details.to_string()),
                    },
                )),
            }),
            durable_stream_id,
            epoch,
            durable_offset: Vec::new(),
        }))
        .await
    }

    /// Reads to the end of a session that sends no stream frames: its result and terminal. Fails
    /// on an output item, a stream end or an input ack, as a non-streaming caller does, on more
    /// than one result, and on any frame after the completion.
    pub async fn collect(self) -> anyhow::Result<SessionOutcome> {
        self.read_to_end(false).await
    }

    /// As [`Self::collect`], but keeps the output items of the session instead of failing on them.
    pub async fn collect_with_output(self) -> anyhow::Result<SessionOutcome> {
        self.read_to_end(true).await
    }

    async fn read_to_end(mut self, allow_output: bool) -> anyhow::Result<SessionOutcome> {
        let mut result = None;
        let mut output_items = Vec::new();
        let mut terminal = None;
        while let Some(response) = self.next().await? {
            if terminal.is_some() {
                bail!("the session sent {response:?} after its completion");
            }
            match response {
                invocation_response::Response::Result(value) => {
                    if result.replace(value).is_some() {
                        bail!("the session sent more than one result");
                    }
                }
                invocation_response::Response::OutputItem(item) if allow_output => {
                    output_items.push(item)
                }
                invocation_response::Response::StreamEnd(_)
                | invocation_response::Response::InputAck(_)
                    if allow_output => {}
                invocation_response::Response::Accepted(_)
                | invocation_response::Response::Ready(_) => {}
                invocation_response::Response::Rejected(rejected) => {
                    terminal = Some(SessionTerminal::Rejected(InvocationError::rejected(
                        rejected,
                    )));
                }
                invocation_response::Response::Finished(finished) => {
                    terminal = Some(match finished.outcome {
                        Some(Outcome::Success(_)) => SessionTerminal::Succeeded,
                        Some(Outcome::Failure(failure)) => {
                            SessionTerminal::Failed(InvocationError::failed(failure))
                        }
                        None => bail!("the session completion has no outcome"),
                    });
                }
                other => bail!("unexpected invocation session frame: {other:?}"),
            }
        }
        let terminal = terminal.ok_or_else(|| anyhow!("the session ended before completion"))?;
        Ok(SessionOutcome {
            result,
            terminal,
            output_items,
            complete: self.is_complete(),
        })
    }

    /// The protocol state saw the whole session. Always `false` for an unvalidated session.
    pub fn is_complete(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(InvocationSessionState::is_complete)
    }

    async fn end_stream(
        &mut self,
        stream: u64,
        sequence: u64,
        direction: StreamDirection,
        outcome: StreamTerminal,
    ) -> anyhow::Result<()> {
        let (durable_stream_id, epoch) = self.mapping(stream)?;
        self.send(invocation_request::Request::StreamEnd(StreamEnd {
            transport_stream_id: stream,
            producer_sequence: sequence,
            direction: direction as i32,
            authored_by: StreamTerminalAuthor::Guest as i32,
            outcome: Some(outcome.try_into().map_err(anyhow::Error::msg)?),
            durable_stream_id,
            epoch,
            durable_offset: Vec::new(),
        }))
        .await
    }

    fn accepted_epoch(&self) -> anyhow::Result<u64> {
        Ok(self
            .accepted
            .as_ref()
            .ok_or_else(|| anyhow!("read the acceptance before sending stream frames"))?
            .epoch)
    }

    fn mapping(
        &self,
        stream: u64,
    ) -> anyhow::Result<(Option<golem_schema::proto::golem::common::Uuid>, u64)> {
        let accepted = self
            .accepted
            .as_ref()
            .ok_or_else(|| anyhow!("read the acceptance before sending stream frames"))?;
        let mapping = accepted
            .stream_mappings
            .iter()
            .find(|mapping| mapping.transport_stream_id == stream)
            .ok_or_else(|| anyhow!("the acceptance has no mapping for stream {stream}"))?;
        Ok((
            mapping.handle.as_ref().and_then(|handle| handle.stream_id),
            accepted.epoch,
        ))
    }
}

/// Reads parts of an [`InvocationSessionResult`].
pub trait InvocationSessionResultExt {
    /// The method result. Fails on a tool result or no result.
    fn method_result(&self) -> anyhow::Result<SchemaValue>;

    /// The mapping of the stream that the first element of a tuple result references.
    fn first_output_stream(&self) -> anyhow::Result<&DurableStreamMapping>;
}

impl InvocationSessionResultExt for InvocationSessionResult {
    fn method_result(&self) -> anyhow::Result<SchemaValue> {
        match &self.result {
            Some(invocation_session_result::Result::MethodResult(value)) => {
                value.clone().try_into().map_err(anyhow::Error::msg)
            }
            other => bail!("expected a method result, got {other:?}"),
        }
    }

    fn first_output_stream(&self) -> anyhow::Result<&DurableStreamMapping> {
        use golem_schema::proto::golem::schema::schema_value::Value;
        let value = match &self.result {
            Some(invocation_session_result::Result::MethodResult(value)) => value,
            other => bail!("expected a method result, got {other:?}"),
        };
        let stream_id = match &value.value {
            Some(Value::TupleValue(tuple)) => match tuple.elements.first() {
                Some(golem_schema::proto::golem::schema::SchemaValue {
                    value: Some(Value::StreamReference(reference)),
                }) => reference.stream_id,
                other => bail!("expected a stream reference first, got {other:?}"),
            },
            other => bail!("expected a tuple result, got {other:?}"),
        };
        self.new_stream_mappings
            .iter()
            .find(|mapping| mapping.transport_stream_id == stream_id)
            .ok_or_else(|| anyhow!("the result omitted the durable mapping of stream {stream_id}"))
    }
}

/// The resume attach of the session that `start` opened and `accepted` accepted: the same
/// identity, auth context, caller and expected fingerprint, a fresh attempt ID, the accepted
/// epoch, `Resume`, and no cursors. Change a field of the result to send another resume.
pub fn resume_attach(start: &InvocationStart, accepted: &InvocationAccepted) -> ResumeAttach {
    ResumeAttach {
        idempotency_key: accepted.idempotency_key,
        agent_id: start_target_id(start),
        environment_id: start.environment_id,
        attachment_id: accepted.attachment_id,
        attempt_id: Some(Uuid::new_v4().into()),
        expected_callee_fingerprint: start.expected_callee_fingerprint,
        expected_epoch: accepted.epoch,
        operation: ResumeOperation::Resume as i32,
        cursors: Vec::new(),
        auth_ctx: start.auth_ctx.clone(),
        caller: start.caller.clone(),
    }
}

/// A reference to transport stream `stream_id` of the session, as a field of
/// [`InvocationStartBuilder::input_record`].
pub fn stream_reference(stream_id: u64) -> ProtoSchemaValue {
    ProtoSchemaValue {
        value: Some(schema_value::Value::StreamReference(
            SchemaValueStreamReference { stream_id },
        )),
    }
}

/// The agent ID of an ID target.
pub fn start_target_id(
    start: &InvocationStart,
) -> Option<golem_api_grpc::proto::golem::worker::AgentId> {
    match start.target.as_ref()?.target.as_ref()? {
        agent_target::Target::Id(agent_id) => Some(*agent_id),
        agent_target::Target::Key(_) => None,
    }
}

/// An invocation target that reaches the agent with this ID.
pub fn id_target(agent_id: &OwnedAgentId) -> AgentTarget {
    AgentTarget {
        target: Some(agent_target::Target::Id(agent_id.agent_id.into())),
    }
}

/// An invocation target that upserts the keyed agent of `key`.
pub fn key_target(key: &ParsedAgentKey) -> anyhow::Result<AgentTarget> {
    Ok(AgentTarget {
        target: Some(agent_target::Target::Key(proto_agent_key(key)?)),
    })
}
