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

use crate::proto::golem::schema::{SchemaValue, result_value, schema_value};
use crate::proto::golem::worker::input_stream_item::Payload;
use crate::proto::golem::worker::{
    AgentId, IdempotencyKey, InputStreamAck, InputStreamEnd, InputStreamItem, InvocationAccepted,
    InvocationFailureKind, InvocationRejectionReason, InvocationRequest, InvocationResponse,
    InvocationSessionCompletion, InvocationSessionResult, PublicInvocationRequest, StreamCancel,
    StreamCancelReason, StreamCancelRole, invocation_request, invocation_response,
    invocation_session_completion, invocation_session_result, public_invocation_request,
};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionPhase {
    Initial,
    AwaitDecision { resume: bool },
    Active,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingAck {
    sequence: u64,
    logical_item_count: u64,
}

#[derive(Debug, Default)]
struct InputState {
    next_offset: u64,
    terminal: bool,
    discard_next_offset: Option<u64>,
    pending_acks: VecDeque<PendingAck>,
}

#[derive(Debug, Default)]
struct OutputState {
    next_offset: u64,
    terminal: bool,
    cancellation_requested: Option<u64>,
}

#[derive(Debug)]
pub struct InvocationSessionState {
    phase: SessionPhase,
    idempotency_key: Option<String>,
    accepted_agent_id: Option<AgentId>,
    accepted_revision: Option<u64>,
    has_result: bool,
    inputs: HashMap<u64, InputState>,
    outputs: HashMap<u64, OutputState>,
}

impl Default for InvocationSessionState {
    fn default() -> Self {
        Self {
            phase: SessionPhase::Initial,
            idempotency_key: None,
            accepted_agent_id: None,
            accepted_revision: None,
            has_result: false,
            inputs: HashMap::new(),
            outputs: HashMap::new(),
        }
    }
}

enum RequestMessage<'a> {
    Start {
        idempotency_key: &'a Option<IdempotencyKey>,
        input: Option<&'a SchemaValue>,
    },
    ResumeAttach {
        idempotency_key: &'a Option<IdempotencyKey>,
    },
    InputItem(&'a InputStreamItem),
    InputEnd(&'a InputStreamEnd),
    StreamCancel(&'a StreamCancel),
}

impl InvocationSessionState {
    pub fn validate_public_request(
        &mut self,
        request: &PublicInvocationRequest,
    ) -> Result<(), String> {
        let message =
            match request.request.as_ref() {
                Some(public_invocation_request::Request::Start(start)) => {
                    if start.application_name.is_empty()
                        || start.environment_name.is_empty()
                        || start.agent_type_name.is_empty()
                        || start.method_name.is_empty()
                    {
                        return Err("public invocation selectors must not be empty".to_string());
                    }
                    if start.constructor_parameters.is_none() {
                        return Err("public invocation has no constructor parameters".to_string());
                    }
                    RequestMessage::Start {
                        idempotency_key: &start.idempotency_key,
                        input: Some(start.method_parameters.as_ref().ok_or_else(|| {
                            "public invocation has no method parameters".to_string()
                        })?),
                    }
                }
                Some(public_invocation_request::Request::ResumeAttach(resume)) => {
                    RequestMessage::ResumeAttach {
                        idempotency_key: &resume.idempotency_key,
                    }
                }
                Some(public_invocation_request::Request::InputItem(item)) => {
                    RequestMessage::InputItem(item)
                }
                Some(public_invocation_request::Request::InputEnd(end)) => {
                    RequestMessage::InputEnd(end)
                }
                Some(public_invocation_request::Request::StreamCancel(cancel)) => {
                    RequestMessage::StreamCancel(cancel)
                }
                None => return Err("invocation request has no payload".to_string()),
            };
        self.validate_request(message)
    }

    pub fn validate_trusted_request(&mut self, request: &InvocationRequest) -> Result<(), String> {
        let message = match request.request.as_ref() {
            Some(invocation_request::Request::Start(start)) => RequestMessage::Start {
                idempotency_key: &start.idempotency_key,
                input: start.input.as_ref(),
            },
            Some(invocation_request::Request::ResumeAttach(resume)) => {
                RequestMessage::ResumeAttach {
                    idempotency_key: &resume.idempotency_key,
                }
            }
            Some(invocation_request::Request::InputItem(item)) => RequestMessage::InputItem(item),
            Some(invocation_request::Request::InputEnd(end)) => RequestMessage::InputEnd(end),
            Some(invocation_request::Request::StreamCancel(cancel)) => {
                RequestMessage::StreamCancel(cancel)
            }
            None => return Err("invocation request has no payload".to_string()),
        };
        self.validate_request(message)
    }

    pub fn validate_response(&mut self, response: &InvocationResponse) -> Result<(), String> {
        if self.phase == SessionPhase::Complete {
            return Err("invocation response received after completion".to_string());
        }
        let response = response
            .response
            .as_ref()
            .ok_or_else(|| "invocation response has no payload".to_string())?;

        match (self.phase, response) {
            (
                SessionPhase::AwaitDecision { resume: false },
                invocation_response::Response::Accepted(accepted),
            ) => self.accept(accepted),
            (
                SessionPhase::AwaitDecision { resume },
                invocation_response::Response::Rejected(rejected),
            ) => {
                let reason =
                    InvocationRejectionReason::try_from(rejected.reason).map_err(|_| {
                        format!("invalid invocation rejection reason {}", rejected.reason)
                    })?;
                if reason == InvocationRejectionReason::Unspecified {
                    return Err("invocation rejection reason is unspecified".to_string());
                }
                if resume && reason != InvocationRejectionReason::ResumeUnsupported {
                    return Err("resume-attach must be rejected as resume-unsupported".to_string());
                }
                self.validate_idempotency_key(&rejected.idempotency_key)?;
                self.phase = SessionPhase::Complete;
                Ok(())
            }
            (SessionPhase::AwaitDecision { resume: true }, _) => {
                Err("resume-attach must receive invocation-rejected".to_string())
            }
            (SessionPhase::AwaitDecision { resume: false }, _) => {
                Err("invocation must be accepted or rejected before other responses".to_string())
            }
            (SessionPhase::Active, invocation_response::Response::Accepted(_)) => {
                Err("invocation response contains more than one acceptance".to_string())
            }
            (SessionPhase::Active, invocation_response::Response::Rejected(_)) => {
                Err("invocation rejection may only appear before acceptance".to_string())
            }
            (SessionPhase::Active, invocation_response::Response::Result(result)) => {
                self.validate_result(result)
            }
            (SessionPhase::Active, invocation_response::Response::OutputItem(item)) => {
                let value = item
                    .value
                    .as_ref()
                    .ok_or_else(|| "output stream item has no value".to_string())?;
                let state = self
                    .outputs
                    .get(&item.stream_id)
                    .ok_or_else(|| format!("output stream {} is unknown", item.stream_id))?;
                ensure_open(state.terminal, item.stream_id)?;
                if item.offset != state.next_offset {
                    return Err(format!(
                        "output stream {} expected offset {}, got {}",
                        item.stream_id, state.next_offset, item.offset
                    ));
                }
                let discovered = stream_references(value)?;
                self.ensure_new_output_streams(&discovered)?;
                let state = self.outputs.get_mut(&item.stream_id).unwrap();
                state.next_offset = state
                    .next_offset
                    .checked_add(1)
                    .ok_or_else(|| format!("output stream {} offset overflow", item.stream_id))?;
                self.insert_output_streams(discovered);
                Ok(())
            }
            (SessionPhase::Active, invocation_response::Response::OutputEnd(end)) => {
                self.terminate_output(end.stream_id, end.offset)
            }
            (SessionPhase::Active, invocation_response::Response::OutputError(error)) => {
                self.terminate_output(error.stream_id, error.offset)
            }
            (SessionPhase::Active, invocation_response::Response::InputAck(ack)) => {
                self.validate_ack(ack)
            }
            (SessionPhase::Active, invocation_response::Response::StreamCancel(cancel)) => {
                validate_cancel(cancel)?;
                match cancel.role() {
                    StreamCancelRole::InputConsumer => {
                        self.cancel_input(cancel.stream_id, cancel.offset, true)
                    }
                    StreamCancelRole::OutputProducer => {
                        self.confirm_output_cancellation(cancel.stream_id, cancel.offset)
                    }
                    _ => Err(
                        "server response may only cancel an input consumer or output producer"
                            .to_string(),
                    ),
                }
            }
            (SessionPhase::Active, invocation_response::Response::AttachmentRevoked(_)) => {
                Err("attachment-revoked is not supported by GOL-91".to_string())
            }
            (SessionPhase::Active, invocation_response::Response::Finished(finished)) => {
                self.finish(finished)
            }
            (SessionPhase::Initial, _) => {
                Err("invocation response received before the first request".to_string())
            }
            (SessionPhase::Complete, _) => unreachable!(),
        }
    }

    pub fn is_complete(&self) -> bool {
        self.phase == SessionPhase::Complete
    }

    fn validate_request(&mut self, message: RequestMessage<'_>) -> Result<(), String> {
        match (self.phase, message) {
            (
                SessionPhase::Initial,
                RequestMessage::Start {
                    idempotency_key,
                    input,
                },
            ) => self.start(idempotency_key, input, false),
            (SessionPhase::Initial, RequestMessage::ResumeAttach { idempotency_key }) => {
                self.start(idempotency_key, None, true)
            }
            (SessionPhase::Initial, _) => {
                Err("the first invocation request must be start or resume-attach".to_string())
            }
            (SessionPhase::AwaitDecision { .. }, _) => {
                Err("invocation input may only be sent after acceptance".to_string())
            }
            (
                SessionPhase::Active,
                RequestMessage::Start { .. } | RequestMessage::ResumeAttach { .. },
            ) => Err(
                "invocation start or resume-attach may only appear as the first request"
                    .to_string(),
            ),
            (SessionPhase::Active, RequestMessage::InputItem(item)) => {
                self.validate_input_item(item)
            }
            (SessionPhase::Active, RequestMessage::InputEnd(end)) => {
                self.terminate_input(end.stream_id, end.offset)
            }
            (SessionPhase::Active, RequestMessage::StreamCancel(cancel)) => {
                validate_cancel(cancel)?;
                match cancel.role() {
                    StreamCancelRole::InputProducer => {
                        self.cancel_input(cancel.stream_id, cancel.offset, false)
                    }
                    StreamCancelRole::OutputConsumer => {
                        self.request_output_cancellation(cancel.stream_id, cancel.offset)
                    }
                    _ => Err(
                        "client request may only cancel an input producer or output consumer"
                            .to_string(),
                    ),
                }
            }
            (SessionPhase::Complete, _) => {
                Err("invocation request received after completion".to_string())
            }
        }
    }

    fn start(
        &mut self,
        idempotency_key: &Option<IdempotencyKey>,
        input: Option<&SchemaValue>,
        resume: bool,
    ) -> Result<(), String> {
        let key = required_idempotency_key(idempotency_key)?;
        let stream_ids = input
            .map(stream_references)
            .transpose()?
            .unwrap_or_default();
        self.inputs = stream_ids
            .into_iter()
            .map(|stream_id| (stream_id, InputState::default()))
            .collect();
        self.idempotency_key = Some(key.to_string());
        self.phase = SessionPhase::AwaitDecision { resume };
        Ok(())
    }

    fn accept(&mut self, accepted: &InvocationAccepted) -> Result<(), String> {
        self.validate_idempotency_key(&accepted.idempotency_key)?;
        let agent_id = accepted
            .agent_id
            .as_ref()
            .ok_or_else(|| "invocation acceptance has no agent identity".to_string())?;
        self.accepted_agent_id = Some(agent_id.clone());
        self.accepted_revision = accepted.component_revision;
        self.phase = SessionPhase::Active;
        Ok(())
    }

    fn validate_result(&mut self, result: &InvocationSessionResult) -> Result<(), String> {
        if self.has_result {
            return Err("invocation response contains more than one result".to_string());
        }
        self.validate_identity(&result.agent_id, &result.idempotency_key)?;
        if let (Some(accepted), Some(result)) = (self.accepted_revision, result.component_revision)
            && accepted != result
        {
            return Err(format!(
                "invocation result revision {result} differs from accepted revision {accepted}"
            ));
        }
        let discovered = match result.result.as_ref() {
            Some(invocation_session_result::Result::MethodResult(value)) => {
                stream_references(value)?
            }
            Some(invocation_session_result::Result::NoResult(_)) => Vec::new(),
            None => return Err("invocation result has no value".to_string()),
        };
        self.ensure_new_output_streams(&discovered)?;
        self.insert_output_streams(discovered);
        self.has_result = true;
        Ok(())
    }

    fn validate_input_item(&mut self, item: &InputStreamItem) -> Result<(), String> {
        let (logical_item_count, discovered) = match item.payload.as_ref() {
            Some(Payload::Value(value)) => (1, stream_references(value)?),
            Some(Payload::PackedU8(bytes)) if !bytes.is_empty() => (bytes.len() as u64, Vec::new()),
            Some(Payload::PackedU8(_)) => {
                return Err("packed-u8 input item must not be empty".to_string());
            }
            None => return Err("input stream item has no payload".to_string()),
        };
        self.ensure_new_input_streams(&discovered)?;
        let state = self
            .inputs
            .get_mut(&item.stream_id)
            .ok_or_else(|| format!("input stream {} is unknown", item.stream_id))?;
        if let Some(discard_next_offset) = state.discard_next_offset.as_mut() {
            if item.sequence != *discard_next_offset {
                return Err(format!(
                    "cancelled input stream {} expected discarded sequence {}, got {}",
                    item.stream_id, *discard_next_offset, item.sequence
                ));
            }
            *discard_next_offset = discard_next_offset
                .checked_add(logical_item_count)
                .ok_or_else(|| format!("input stream {} offset overflow", item.stream_id))?;
            self.insert_input_streams(discovered);
            return Ok(());
        }
        ensure_open(state.terminal, item.stream_id)?;
        if item.sequence != state.next_offset {
            return Err(format!(
                "input stream {} expected sequence {}, got {}",
                item.stream_id, state.next_offset, item.sequence
            ));
        }
        let next_offset = state
            .next_offset
            .checked_add(logical_item_count)
            .ok_or_else(|| format!("input stream {} offset overflow", item.stream_id))?;
        state.pending_acks.push_back(PendingAck {
            sequence: item.sequence,
            logical_item_count,
        });
        state.next_offset = next_offset;
        self.insert_input_streams(discovered);
        Ok(())
    }

    fn validate_ack(&mut self, ack: &InputStreamAck) -> Result<(), String> {
        let state = self
            .inputs
            .get_mut(&ack.stream_id)
            .ok_or_else(|| format!("input stream {} is unknown", ack.stream_id))?;
        let expected = state.pending_acks.front().ok_or_else(|| {
            format!(
                "input stream {} has no item awaiting acknowledgement",
                ack.stream_id
            )
        })?;
        if ack.sequence != expected.sequence
            || ack.logical_item_count != expected.logical_item_count
        {
            return Err(format!(
                "input stream {} expected acknowledgement ({}, {}), got ({}, {})",
                ack.stream_id,
                expected.sequence,
                expected.logical_item_count,
                ack.sequence,
                ack.logical_item_count
            ));
        }
        state.pending_acks.pop_front();
        Ok(())
    }

    fn terminate_input(&mut self, stream_id: u64, offset: u64) -> Result<(), String> {
        let state = self
            .inputs
            .get_mut(&stream_id)
            .ok_or_else(|| format!("input stream {stream_id} is unknown"))?;
        if let Some(discard_next_offset) = state.discard_next_offset {
            if offset != discard_next_offset {
                return Err(format!(
                    "cancelled input stream {stream_id} expected discarded terminal offset {discard_next_offset}, got {offset}"
                ));
            }
            state.discard_next_offset = None;
            return Ok(());
        }
        ensure_open(state.terminal, stream_id)?;
        if offset != state.next_offset {
            return Err(format!(
                "input stream {stream_id} expected terminal offset {}, got {offset}",
                state.next_offset
            ));
        }
        state.terminal = true;
        Ok(())
    }

    fn cancel_input(
        &mut self,
        stream_id: u64,
        offset: u64,
        discard_in_flight: bool,
    ) -> Result<(), String> {
        let state = self
            .inputs
            .get_mut(&stream_id)
            .ok_or_else(|| format!("input stream {stream_id} is unknown"))?;
        ensure_open(state.terminal, stream_id)?;
        let accepted_offset = state
            .pending_acks
            .front()
            .map(|pending| pending.sequence)
            .unwrap_or(state.next_offset);
        if offset != accepted_offset {
            return Err(format!(
                "input stream {stream_id} expected cancellation offset {accepted_offset}, got {offset}"
            ));
        }
        if discard_in_flight {
            state.discard_next_offset = Some(state.next_offset);
        }
        state.next_offset = offset;
        state.pending_acks.clear();
        state.terminal = true;
        Ok(())
    }

    fn terminate_output(&mut self, stream_id: u64, offset: u64) -> Result<(), String> {
        let state = self
            .outputs
            .get_mut(&stream_id)
            .ok_or_else(|| format!("output stream {stream_id} is unknown"))?;
        ensure_open(state.terminal, stream_id)?;
        if offset != state.next_offset {
            return Err(format!(
                "output stream {stream_id} expected terminal offset {}, got {offset}",
                state.next_offset
            ));
        }
        state.terminal = true;
        Ok(())
    }

    fn request_output_cancellation(&mut self, stream_id: u64, offset: u64) -> Result<(), String> {
        let state = self
            .outputs
            .get_mut(&stream_id)
            .ok_or_else(|| format!("output stream {stream_id} is unknown"))?;
        ensure_open(state.terminal, stream_id)?;
        if state.cancellation_requested.is_some() {
            return Err(format!(
                "output stream {stream_id} already has a pending consumer cancellation"
            ));
        }
        if offset > state.next_offset {
            return Err(format!(
                "output stream {stream_id} cannot cancel at future offset {offset}; latest observed offset is {}",
                state.next_offset
            ));
        }
        state.cancellation_requested = Some(offset);
        Ok(())
    }

    fn confirm_output_cancellation(&mut self, stream_id: u64, offset: u64) -> Result<(), String> {
        let requested = {
            let state = self
                .outputs
                .get(&stream_id)
                .ok_or_else(|| format!("output stream {stream_id} is unknown"))?;
            ensure_open(state.terminal, stream_id)?;
            state.cancellation_requested
        };
        match requested {
            Some(requested_offset) if offset != requested_offset => Err(format!(
                "output stream {stream_id} expected cancellation confirmation at offset {requested_offset}, got {offset}"
            )),
            Some(_) => {
                self.outputs
                    .get_mut(&stream_id)
                    .expect("output stream disappeared while confirming cancellation")
                    .terminal = true;
                Ok(())
            }
            None => self.terminate_output(stream_id, offset),
        }
    }

    fn finish(&mut self, finished: &InvocationSessionCompletion) -> Result<(), String> {
        match finished.outcome.as_ref() {
            Some(invocation_session_completion::Outcome::Success(_)) if !self.has_result => {
                return Err(
                    "invocation completed successfully before publishing a result".to_string(),
                );
            }
            Some(invocation_session_completion::Outcome::Failure(failure)) => {
                let kind = InvocationFailureKind::try_from(failure.kind)
                    .map_err(|_| format!("invalid invocation failure kind {}", failure.kind))?;
                if kind == InvocationFailureKind::Unspecified {
                    return Err("invocation failure kind is unspecified".to_string());
                }
                if failure.worker_error.is_some() && kind != InvocationFailureKind::Execution {
                    return Err(
                        "worker execution details require an execution failure kind".to_string()
                    );
                }
            }
            Some(invocation_session_completion::Outcome::Success(_)) => {}
            None => return Err("invocation completion has no outcome".to_string()),
        }
        let unterminated_inputs = self
            .inputs
            .iter()
            .filter_map(|(stream_id, state)| (!state.terminal).then_some(*stream_id))
            .collect::<Vec<_>>();
        if !unterminated_inputs.is_empty() {
            return Err(format!(
                "invocation completed before input streams terminated: {unterminated_inputs:?}"
            ));
        }
        let unacknowledged_inputs = self
            .inputs
            .iter()
            .filter_map(|(stream_id, state)| (!state.pending_acks.is_empty()).then_some(*stream_id))
            .collect::<Vec<_>>();
        if !unacknowledged_inputs.is_empty() {
            return Err(format!(
                "invocation completed with unacknowledged input streams: {unacknowledged_inputs:?}"
            ));
        }
        let unterminated_outputs = self
            .outputs
            .iter()
            .filter_map(|(stream_id, state)| (!state.terminal).then_some(*stream_id))
            .collect::<Vec<_>>();
        if !unterminated_outputs.is_empty() {
            return Err(format!(
                "invocation completed before output streams terminated: {unterminated_outputs:?}"
            ));
        }
        self.phase = SessionPhase::Complete;
        Ok(())
    }

    fn validate_idempotency_key(&self, key: &Option<IdempotencyKey>) -> Result<(), String> {
        let actual = required_idempotency_key(key)?;
        let expected = self
            .idempotency_key
            .as_deref()
            .ok_or_else(|| "invocation has no idempotency key".to_string())?;
        if actual != expected {
            return Err(format!(
                "invocation idempotency key mismatch: expected {expected}, got {actual}"
            ));
        }
        Ok(())
    }

    fn validate_identity(
        &self,
        agent_id: &Option<AgentId>,
        idempotency_key: &Option<IdempotencyKey>,
    ) -> Result<(), String> {
        self.validate_idempotency_key(idempotency_key)?;
        let actual = agent_id
            .as_ref()
            .ok_or_else(|| "invocation result has no agent identity".to_string())?;
        let expected = self
            .accepted_agent_id
            .as_ref()
            .ok_or_else(|| "invocation has no accepted agent identity".to_string())?;
        if actual != expected {
            return Err("invocation result agent identity differs from acceptance".to_string());
        }
        Ok(())
    }

    fn ensure_new_input_streams(&self, stream_ids: &[u64]) -> Result<(), String> {
        for stream_id in stream_ids {
            if self.inputs.contains_key(stream_id) || self.outputs.contains_key(stream_id) {
                return Err(format!("stream {stream_id} is already registered"));
            }
        }
        Ok(())
    }

    fn insert_input_streams(&mut self, stream_ids: Vec<u64>) {
        for stream_id in stream_ids {
            self.inputs.insert(stream_id, InputState::default());
        }
    }

    fn ensure_new_output_streams(&self, stream_ids: &[u64]) -> Result<(), String> {
        for stream_id in stream_ids {
            if self.inputs.contains_key(stream_id) || self.outputs.contains_key(stream_id) {
                return Err(format!("stream {stream_id} is already registered"));
            }
        }
        Ok(())
    }

    fn insert_output_streams(&mut self, stream_ids: Vec<u64>) {
        for stream_id in stream_ids {
            self.outputs.insert(stream_id, OutputState::default());
        }
    }
}

fn required_idempotency_key(key: &Option<IdempotencyKey>) -> Result<&str, String> {
    let value = key
        .as_ref()
        .ok_or_else(|| "invocation has no idempotency key".to_string())?
        .value
        .as_str();
    if value.is_empty() {
        Err("invocation idempotency key is empty".to_string())
    } else {
        Ok(value)
    }
}

fn validate_cancel(cancel: &StreamCancel) -> Result<(), String> {
    let role = StreamCancelRole::try_from(cancel.role)
        .map_err(|_| format!("invalid stream cancellation role {}", cancel.role))?;
    if role == StreamCancelRole::Unspecified {
        return Err("stream cancellation role is unspecified".to_string());
    }
    let reason = StreamCancelReason::try_from(cancel.reason)
        .map_err(|_| format!("invalid stream cancellation reason {}", cancel.reason))?;
    if reason == StreamCancelReason::Unspecified {
        return Err("stream cancellation reason is unspecified".to_string());
    }
    Ok(())
}

fn ensure_open(terminal: bool, stream_id: u64) -> Result<(), String> {
    if terminal {
        Err(format!(
            "stream {stream_id} received an event after its terminal"
        ))
    } else {
        Ok(())
    }
}

fn stream_references(value: &SchemaValue) -> Result<Vec<u64>, String> {
    fn visit(
        value: &SchemaValue,
        stream_ids: &mut Vec<u64>,
        unique: &mut HashSet<u64>,
    ) -> Result<(), String> {
        match value
            .value
            .as_ref()
            .ok_or_else(|| "schema value has no payload".to_string())?
        {
            schema_value::Value::RecordValue(record) => {
                for field in &record.fields {
                    visit(field, stream_ids, unique)?;
                }
            }
            schema_value::Value::VariantValue(variant) => {
                if let Some(payload) = variant.payload.as_deref() {
                    visit(payload, stream_ids, unique)?;
                }
            }
            schema_value::Value::TupleValue(tuple) => {
                for element in &tuple.elements {
                    visit(element, stream_ids, unique)?;
                }
            }
            schema_value::Value::ListValue(list) => {
                for element in &list.elements {
                    visit(element, stream_ids, unique)?;
                }
            }
            schema_value::Value::FixedListValue(list) => {
                for element in &list.elements {
                    visit(element, stream_ids, unique)?;
                }
            }
            schema_value::Value::MapValue(map) => {
                for entry in &map.entries {
                    if let Some(key) = entry.key.as_ref() {
                        visit(key, stream_ids, unique)?;
                    }
                    if let Some(value) = entry.value.as_ref() {
                        visit(value, stream_ids, unique)?;
                    }
                }
            }
            schema_value::Value::OptionValue(option) => {
                if let Some(inner) = option.inner.as_deref() {
                    visit(inner, stream_ids, unique)?;
                }
            }
            schema_value::Value::ResultValue(result) => match result.result.as_ref() {
                Some(result_value::Result::Ok(value) | result_value::Result::Err(value)) => {
                    visit(value, stream_ids, unique)?;
                }
                Some(result_value::Result::OkUnit(_) | result_value::Result::ErrUnit(_)) => {}
                None => return Err("result schema value has no payload".to_string()),
            },
            schema_value::Value::UnionValue(union) => {
                if let Some(body) = union.body.as_deref() {
                    visit(body, stream_ids, unique)?;
                }
            }
            schema_value::Value::StreamReference(reference) => {
                if !unique.insert(reference.stream_id) {
                    return Err(format!(
                        "stream {} is referenced more than once",
                        reference.stream_id
                    ));
                }
                stream_ids.push(reference.stream_id);
            }
            _ => {}
        }
        Ok(())
    }

    let mut stream_ids = Vec::new();
    visit(value, &mut stream_ids, &mut HashSet::new())?;
    Ok(stream_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::golem::common::Empty;
    use crate::proto::golem::schema::{RecordValue, SchemaValueStreamReference};
    use crate::proto::golem::worker::{
        InvocationFailure, InvocationRejected, InvocationStart, OutputStreamEnd, OutputStreamError,
        OutputStreamItem, PublicInvocationStart, ResumeAttach,
    };
    use prost::Message;
    use test_r::test;

    const KEY: &str = "session-key";

    fn key() -> Option<IdempotencyKey> {
        Some(IdempotencyKey {
            value: KEY.to_string(),
        })
    }

    fn scalar(value: u32) -> SchemaValue {
        SchemaValue {
            value: Some(schema_value::Value::U8Value(value)),
        }
    }

    fn stream(stream_id: u64) -> SchemaValue {
        SchemaValue {
            value: Some(schema_value::Value::StreamReference(
                SchemaValueStreamReference { stream_id },
            )),
        }
    }

    fn record(fields: Vec<SchemaValue>) -> SchemaValue {
        SchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue { fields })),
        }
    }

    fn public_request(request: public_invocation_request::Request) -> PublicInvocationRequest {
        PublicInvocationRequest {
            request: Some(request),
        }
    }

    fn trusted_request(request: invocation_request::Request) -> InvocationRequest {
        InvocationRequest {
            request: Some(request),
        }
    }

    fn response(response: invocation_response::Response) -> InvocationResponse {
        InvocationResponse {
            response: Some(response),
        }
    }

    fn public_start(input: SchemaValue) -> PublicInvocationRequest {
        public_request(public_invocation_request::Request::Start(
            PublicInvocationStart {
                application_name: "app".to_string(),
                environment_name: "env".to_string(),
                agent_type_name: "agent-type".to_string(),
                constructor_parameters: Some(record(Vec::new())),
                method_name: "run".to_string(),
                method_parameters: Some(input),
                idempotency_key: key(),
                ..Default::default()
            },
        ))
    }

    fn trusted_start(input: SchemaValue) -> InvocationRequest {
        trusted_request(invocation_request::Request::Start(InvocationStart {
            input: Some(input),
            idempotency_key: key(),
            ..Default::default()
        }))
    }

    fn agent_id() -> AgentId {
        AgentId {
            component_id: None,
            name: "agent".to_string(),
        }
    }

    fn accepted() -> InvocationResponse {
        response(invocation_response::Response::Accepted(
            InvocationAccepted {
                agent_id: Some(agent_id()),
                idempotency_key: key(),
                component_revision: Some(12),
            },
        ))
    }

    fn result(value: SchemaValue) -> InvocationResponse {
        response(invocation_response::Response::Result(
            InvocationSessionResult {
                result: Some(invocation_session_result::Result::MethodResult(value)),
                component_revision: Some(12),
                agent_id: Some(agent_id()),
                idempotency_key: key(),
                ..Default::default()
            },
        ))
    }

    fn success() -> InvocationResponse {
        response(invocation_response::Response::Finished(
            InvocationSessionCompletion {
                outcome: Some(invocation_session_completion::Outcome::Success(Empty {})),
            },
        ))
    }

    fn failure(kind: InvocationFailureKind) -> InvocationResponse {
        response(invocation_response::Response::Finished(
            InvocationSessionCompletion {
                outcome: Some(invocation_session_completion::Outcome::Failure(
                    InvocationFailure {
                        kind: kind as i32,
                        code: "failed".to_string(),
                        message: "invocation failed".to_string(),
                        worker_error: (kind == InvocationFailureKind::Execution)
                            .then(Default::default),
                    },
                )),
            },
        ))
    }

    fn input_item(sequence: u64, payload: Payload) -> PublicInvocationRequest {
        public_request(public_invocation_request::Request::InputItem(
            InputStreamItem {
                stream_id: 7,
                sequence,
                payload: Some(payload),
            },
        ))
    }

    fn cancel(stream_id: u64, role: StreamCancelRole, offset: u64) -> StreamCancel {
        StreamCancel {
            stream_id,
            offset,
            role: role as i32,
            reason: StreamCancelReason::Cancelled as i32,
            details: None,
        }
    }

    #[test]
    fn legal_public_session_tracks_recursive_streams_acks_and_finish() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(vec![stream(7)])))
            .unwrap();
        assert!(
            state
                .validate_public_request(&input_item(0, Payload::PackedU8(vec![1, 2])))
                .is_err()
        );
        state.validate_response(&accepted()).unwrap();
        state
            .validate_public_request(&input_item(0, Payload::PackedU8(vec![1, 2])))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::InputAck(
                InputStreamAck {
                    stream_id: 7,
                    sequence: 0,
                    logical_item_count: 2,
                },
            )))
            .unwrap();
        state
            .validate_public_request(&public_request(
                public_invocation_request::Request::InputEnd(InputStreamEnd {
                    stream_id: 7,
                    offset: 2,
                }),
            ))
            .unwrap();
        state
            .validate_response(&result(record(vec![stream(9), stream(10)])))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::OutputItem(
                OutputStreamItem {
                    stream_id: 9,
                    offset: 0,
                    value: Some(record(vec![stream(11)])),
                },
            )))
            .unwrap();
        for (stream_id, offset) in [(9, 1), (10, 0), (11, 0)] {
            state
                .validate_response(&response(invocation_response::Response::OutputEnd(
                    OutputStreamEnd { stream_id, offset },
                )))
                .unwrap();
        }
        state.validate_response(&success()).unwrap();
        assert!(state.is_complete());
        assert!(state.validate_response(&result(scalar(1))).is_err());
    }

    #[test]
    fn legal_trusted_stream_free_session_is_accepted_result_finished() {
        let mut state = InvocationSessionState::default();
        state
            .validate_trusted_request(&trusted_start(record(Vec::new())))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        state.validate_response(&result(scalar(1))).unwrap();
        state.validate_response(&success()).unwrap();
    }

    #[test]
    fn resume_attach_requires_terminal_resume_unsupported_rejection() {
        let resume = public_request(public_invocation_request::Request::ResumeAttach(
            ResumeAttach {
                idempotency_key: key(),
            },
        ));
        let mut state = InvocationSessionState::default();
        state.validate_public_request(&resume).unwrap();
        assert!(state.validate_response(&accepted()).is_err());
        assert!(
            state
                .validate_response(&response(invocation_response::Response::Rejected(
                    InvocationRejected {
                        reason: InvocationRejectionReason::Validation as i32,
                        idempotency_key: key(),
                        ..Default::default()
                    },
                )))
                .is_err()
        );
        state
            .validate_response(&response(invocation_response::Response::Rejected(
                InvocationRejected {
                    reason: InvocationRejectionReason::ResumeUnsupported as i32,
                    idempotency_key: key(),
                    ..Default::default()
                },
            )))
            .unwrap();
        assert!(state.is_complete());
        assert!(state.validate_public_request(&resume).is_err());
    }

    #[test]
    fn rejection_is_terminal_for_both_directions() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::Rejected(
                InvocationRejected {
                    reason: InvocationRejectionReason::Validation as i32,
                    idempotency_key: key(),
                    ..Default::default()
                },
            )))
            .unwrap();
        assert!(state.validate_response(&accepted()).is_err());
        assert!(
            state
                .validate_public_request(&public_request(
                    public_invocation_request::Request::InputEnd(InputStreamEnd::default()),
                ))
                .is_err()
        );
    }

    #[test]
    fn unknown_streams_ack_mismatch_and_post_terminal_events_are_rejected() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        assert!(
            state
                .validate_public_request(&public_request(
                    public_invocation_request::Request::InputEnd(InputStreamEnd {
                        stream_id: 8,
                        offset: 0,
                    }),
                ))
                .is_err()
        );
        state
            .validate_public_request(&input_item(0, Payload::PackedU8(vec![1, 2, 3])))
            .unwrap();
        assert!(
            state
                .validate_response(&response(invocation_response::Response::InputAck(
                    InputStreamAck {
                        stream_id: 7,
                        sequence: 0,
                        logical_item_count: 2,
                    },
                )))
                .is_err()
        );
        state
            .validate_response(&response(invocation_response::Response::InputAck(
                InputStreamAck {
                    stream_id: 7,
                    sequence: 0,
                    logical_item_count: 3,
                },
            )))
            .unwrap();
        state
            .validate_public_request(&public_request(
                public_invocation_request::Request::InputEnd(InputStreamEnd {
                    stream_id: 7,
                    offset: 3,
                }),
            ))
            .unwrap();
        assert!(
            state
                .validate_public_request(&input_item(3, Payload::Value(scalar(1))))
                .is_err()
        );
    }

    #[test]
    fn output_consumer_cancellation_after_stream_terminal_is_rejected() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        state.validate_response(&result(stream(9))).unwrap();
        state
            .validate_response(&response(invocation_response::Response::OutputEnd(
                OutputStreamEnd {
                    stream_id: 9,
                    offset: 0,
                },
            )))
            .unwrap();

        assert!(
            state
                .validate_public_request(&public_request(
                    public_invocation_request::Request::StreamCancel(cancel(
                        9,
                        StreamCancelRole::OutputConsumer,
                        0,
                    )),
                ))
                .is_err(),
            "an output-consumer cancellation must not be accepted after the stream terminal"
        );
    }

    #[test]
    fn input_items_register_recursively_nested_streams() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        state
            .validate_public_request(&input_item(0, Payload::Value(record(vec![stream(8)]))))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::InputAck(
                InputStreamAck {
                    stream_id: 7,
                    sequence: 0,
                    logical_item_count: 1,
                },
            )))
            .unwrap();
        state
            .validate_public_request(&public_request(
                public_invocation_request::Request::InputEnd(InputStreamEnd {
                    stream_id: 7,
                    offset: 1,
                }),
            ))
            .unwrap();
        state
            .validate_public_request(&public_request(
                public_invocation_request::Request::InputEnd(InputStreamEnd {
                    stream_id: 8,
                    offset: 0,
                }),
            ))
            .unwrap();
        state.validate_response(&result(scalar(1))).unwrap();
        state.validate_response(&success()).unwrap();
    }

    #[test]
    fn all_role_appropriate_cancellations_are_unique_terminals() {
        let mut input = InvocationSessionState::default();
        input
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        input.validate_response(&accepted()).unwrap();
        let input_cancel = public_request(public_invocation_request::Request::StreamCancel(
            cancel(7, StreamCancelRole::InputProducer, 0),
        ));
        input.validate_public_request(&input_cancel).unwrap();
        assert!(input.validate_public_request(&input_cancel).is_err());

        let mut input_consumer = InvocationSessionState::default();
        input_consumer
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        input_consumer.validate_response(&accepted()).unwrap();
        input_consumer
            .validate_response(&response(invocation_response::Response::StreamCancel(
                cancel(7, StreamCancelRole::InputConsumer, 0),
            )))
            .unwrap();

        let mut output = InvocationSessionState::default();
        output
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        output.validate_response(&accepted()).unwrap();
        output.validate_response(&result(stream(9))).unwrap();
        output
            .validate_public_request(&public_request(
                public_invocation_request::Request::StreamCancel(cancel(
                    9,
                    StreamCancelRole::OutputConsumer,
                    0,
                )),
            ))
            .unwrap();
        assert!(
            output
                .validate_public_request(&public_request(
                    public_invocation_request::Request::StreamCancel(cancel(
                        9,
                        StreamCancelRole::OutputConsumer,
                        0,
                    )),
                ))
                .is_err()
        );
        output
            .validate_response(&response(invocation_response::Response::OutputItem(
                OutputStreamItem {
                    stream_id: 9,
                    offset: 0,
                    value: Some(scalar(1)),
                },
            )))
            .unwrap();
        output
            .validate_response(&response(invocation_response::Response::StreamCancel(
                cancel(9, StreamCancelRole::OutputProducer, 0),
            )))
            .unwrap();
        output.validate_response(&success()).unwrap();

        let mut output_producer = InvocationSessionState::default();
        output_producer
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        output_producer.validate_response(&accepted()).unwrap();
        output_producer
            .validate_response(&result(stream(9)))
            .unwrap();
        output_producer
            .validate_response(&response(invocation_response::Response::StreamCancel(
                cancel(9, StreamCancelRole::OutputProducer, 0),
            )))
            .unwrap();
    }

    #[test]
    fn input_consumer_cancellation_abandons_an_unacknowledged_item() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        state
            .validate_public_request(&input_item(0, Payload::Value(scalar(1))))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::StreamCancel(
                cancel(7, StreamCancelRole::InputConsumer, 0),
            )))
            .unwrap();
        state
            .validate_public_request(&input_item(1, Payload::Value(scalar(2))))
            .unwrap();
        state
            .validate_public_request(&public_request(
                public_invocation_request::Request::InputEnd(InputStreamEnd {
                    stream_id: 7,
                    offset: 2,
                }),
            ))
            .unwrap();
        state.validate_response(&result(scalar(2))).unwrap();
        state.validate_response(&success()).unwrap();
    }

    #[test]
    fn duplicate_input_end_after_consumer_cancellation_is_rejected() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        state
            .validate_response(&response(invocation_response::Response::StreamCancel(
                cancel(7, StreamCancelRole::InputConsumer, 0),
            )))
            .unwrap();
        let end = public_request(public_invocation_request::Request::InputEnd(
            InputStreamEnd {
                stream_id: 7,
                offset: 0,
            },
        ));
        state.validate_public_request(&end).unwrap();

        assert!(
            state.validate_public_request(&end).is_err(),
            "an input stream must not accept the producer terminal more than once"
        );
    }

    #[test]
    fn recursive_registration_is_transactional_and_attachment_revocation_is_illegal() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        assert!(
            state
                .validate_response(&result(record(vec![stream(9), stream(9)])))
                .is_err()
        );
        state.validate_response(&result(stream(9))).unwrap();
        assert!(
            state
                .validate_response(&response(invocation_response::Response::OutputItem(
                    OutputStreamItem {
                        stream_id: 9,
                        offset: 0,
                        value: Some(stream(9)),
                    },
                )))
                .is_err()
        );
        state
            .validate_response(&response(invocation_response::Response::OutputItem(
                OutputStreamItem {
                    stream_id: 9,
                    offset: 0,
                    value: Some(scalar(1)),
                },
            )))
            .unwrap();
        assert!(
            state
                .validate_response(&response(invocation_response::Response::AttachmentRevoked(
                    Default::default()
                ),))
                .is_err()
        );
    }

    #[test]
    fn stream_ids_are_unique_across_input_and_output_directions() {
        let mut input_first = InvocationSessionState::default();
        input_first
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        input_first.validate_response(&accepted()).unwrap();
        assert!(input_first.validate_response(&result(stream(7))).is_err());

        let mut output_first = InvocationSessionState::default();
        output_first
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        output_first.validate_response(&accepted()).unwrap();
        output_first.validate_response(&result(stream(9))).unwrap();
        assert!(
            output_first
                .validate_public_request(&input_item(0, Payload::Value(stream(9))))
                .is_err()
        );
    }

    #[test]
    fn finish_requires_result_terminals_and_safe_failure_kind() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        assert!(state.validate_response(&success()).is_err());
        assert!(
            state
                .validate_response(&failure(InvocationFailureKind::Unspecified))
                .is_err()
        );
        let mut protocol_with_worker_error = failure(InvocationFailureKind::Protocol);
        let Some(invocation_response::Response::Finished(InvocationSessionCompletion {
            outcome: Some(invocation_session_completion::Outcome::Failure(protocol_failure)),
        })) = protocol_with_worker_error.response.as_mut()
        else {
            unreachable!("failure helper returned the wrong response")
        };
        protocol_failure.worker_error = Some(Default::default());
        assert!(
            state
                .validate_response(&protocol_with_worker_error)
                .is_err()
        );
        state
            .validate_response(&failure(InvocationFailureKind::Execution))
            .unwrap();
    }

    fn round_trip<M>(message: M)
    where
        M: Message + Default + PartialEq + std::fmt::Debug,
    {
        let encoded = message.encode_to_vec();
        assert_eq!(M::decode(encoded.as_slice()).unwrap(), message);
    }

    #[test]
    fn public_and_internal_envelopes_round_trip_all_gol_91_variants() {
        round_trip(public_start(record(Vec::new())));
        round_trip(trusted_start(record(Vec::new())));
        round_trip(public_request(
            public_invocation_request::Request::ResumeAttach(ResumeAttach {
                idempotency_key: key(),
            }),
        ));
        round_trip(input_item(4, Payload::PackedU8(vec![1, 2, 3])));
        round_trip(public_request(
            public_invocation_request::Request::InputEnd(InputStreamEnd {
                stream_id: 7,
                offset: 4,
            }),
        ));
        for role in [
            StreamCancelRole::InputProducer,
            StreamCancelRole::InputConsumer,
            StreamCancelRole::OutputProducer,
            StreamCancelRole::OutputConsumer,
        ] {
            round_trip(StreamCancel {
                stream_id: 7,
                offset: 8,
                role: role as i32,
                reason: StreamCancelReason::Protocol as i32,
                details: Some("cancelled".to_string()),
            });
        }
        round_trip(accepted());
        round_trip(response(invocation_response::Response::Rejected(
            InvocationRejected {
                reason: InvocationRejectionReason::ResumeUnsupported as i32,
                idempotency_key: key(),
                ..Default::default()
            },
        )));
        round_trip(result(stream(9)));
        round_trip(response(invocation_response::Response::OutputItem(
            OutputStreamItem {
                stream_id: 9,
                offset: 0,
                value: Some(scalar(1)),
            },
        )));
        round_trip(response(invocation_response::Response::OutputEnd(
            OutputStreamEnd {
                stream_id: 9,
                offset: 1,
            },
        )));
        round_trip(response(invocation_response::Response::OutputError(
            OutputStreamError {
                stream_id: 9,
                offset: 1,
                details: "failed".to_string(),
            },
        )));
        round_trip(response(invocation_response::Response::InputAck(
            InputStreamAck {
                stream_id: 7,
                sequence: 0,
                logical_item_count: 3,
            },
        )));
        round_trip(response(invocation_response::Response::StreamCancel(
            cancel(9, StreamCancelRole::OutputProducer, 1),
        )));
        round_trip(response(invocation_response::Response::AttachmentRevoked(
            Default::default(),
        )));
        round_trip(success());
        round_trip(failure(InvocationFailureKind::Protocol));
        round_trip(failure(InvocationFailureKind::Execution));
    }
}
