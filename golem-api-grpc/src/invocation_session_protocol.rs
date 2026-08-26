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
    AgentId, DurableStreamHandle, DurableStreamMapping, IdempotencyKey, InputStreamAck,
    InputStreamEnd, InputStreamItem, InvocationAccepted, InvocationFailureKind,
    InvocationRejectionReason, InvocationRequest, InvocationResponse, InvocationSessionCompletion,
    InvocationSessionResult, PublicInvocationRequest, ResumeAttach, ResumeOperation, StreamCancel,
    StreamCancelReason, StreamCancelRole, StreamMappingRole, invocation_request,
    invocation_response, invocation_session_completion, invocation_session_result,
    public_invocation_request,
};
use prost::Message;
use std::collections::{HashMap, HashSet, VecDeque};

type StreamBinding = (u64, (u64, u64));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionPhase {
    Initial,
    AwaitDecision { resume: bool },
    Active,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InputItemIdentity {
    sequence: u64,
    logical_item_count: u64,
    new_stream_ids: Vec<u64>,
    payload_fingerprint: [u8; 32],
    terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingInputRange {
    item: InputItemIdentity,
    historical: bool,
    durable_offset: Option<[u8; 24]>,
    outstanding_acks: u64,
}

#[derive(Debug, Default)]
struct InputState {
    next_offset: u64,
    terminal: bool,
    discard_next_offset: Option<u64>,
    pending_ranges: HashMap<u64, PendingInputRange>,
    pending_acks: VecDeque<u64>,
    durable_stream_id: Option<(u64, u64)>,
    last_durable_offset: Option<[u8; 24]>,
}

#[derive(Debug, Default)]
struct OutputState {
    next_offset: u64,
    resume_first_frame: bool,
    resume_mapping_announcement_pending: bool,
    terminal: bool,
    cancellation_requested: Option<u64>,
    durable_stream_id: (u64, u64),
    last_durable_offset: Option<[u8; 24]>,
}

#[derive(Debug, Clone, Copy)]
struct ValidatedOutputFrame {
    durable_offset: [u8; 24],
}

#[derive(Debug)]
pub struct InvocationSessionState {
    phase: SessionPhase,
    idempotency_key: Option<String>,
    accepted_agent_id: Option<AgentId>,
    accepted_revision: Option<u64>,
    accepted_epoch: Option<u64>,
    resume: bool,
    resume_cursors: HashMap<(u64, u64), Option<[u8; 24]>>,
    resume_agent_id: Option<AgentId>,
    resume_environment_id: Option<crate::proto::golem::common::EnvironmentId>,
    resume_attachment_id: Option<crate::proto::golem::common::Uuid>,
    resume_attempt_id: Option<crate::proto::golem::common::Uuid>,
    resume_callee_fingerprint: Option<crate::proto::golem::common::Uuid>,
    resume_accepted_epoch: Option<u64>,
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
            accepted_epoch: None,
            resume: false,
            resume_cursors: HashMap::new(),
            resume_agent_id: None,
            resume_environment_id: None,
            resume_attachment_id: None,
            resume_attempt_id: None,
            resume_callee_fingerprint: None,
            resume_accepted_epoch: None,
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
        out_of_band_inputs: bool,
    },
    ResumeAttach {
        idempotency_key: &'a Option<IdempotencyKey>,
        resume: &'a ResumeAttach,
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
        self.validate_public_request_with_terminal_race(request, false)
    }

    /// Validates a public request at the receiving end of a full-duplex session.
    ///
    /// A consumer may send an output cancellation before observing a terminal response that the
    /// receiver has already recorded. This accepts that cancellation once while preserving strict
    /// validation for locally generated requests.
    pub fn validate_received_public_request(
        &mut self,
        request: &PublicInvocationRequest,
    ) -> Result<(), String> {
        self.validate_public_request_with_terminal_race(request, true)
    }

    fn validate_public_request_with_terminal_race(
        &mut self,
        request: &PublicInvocationRequest,
        accept_terminal_output_cancellation: bool,
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
                        out_of_band_inputs: false,
                    }
                }
                Some(public_invocation_request::Request::ResumeAttach(resume)) => {
                    RequestMessage::ResumeAttach {
                        idempotency_key: &resume.idempotency_key,
                        resume,
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
        self.validate_request(message, accept_terminal_output_cancellation)
    }

    pub fn validate_trusted_request(&mut self, request: &InvocationRequest) -> Result<(), String> {
        self.validate_trusted_request_with_terminal_race(request, false)
    }

    /// Validates a trusted request at the receiving end of a full-duplex session.
    ///
    /// A consumer may send an output cancellation before observing a terminal response that the
    /// receiver has already recorded. This accepts that cancellation once while preserving strict
    /// validation for locally generated requests.
    pub fn validate_received_trusted_request(
        &mut self,
        request: &InvocationRequest,
    ) -> Result<(), String> {
        self.validate_trusted_request_with_terminal_race(request, true)
    }

    fn validate_trusted_request_with_terminal_race(
        &mut self,
        request: &InvocationRequest,
        accept_terminal_output_cancellation: bool,
    ) -> Result<(), String> {
        let message = match request.request.as_ref() {
            Some(invocation_request::Request::Start(start)) => RequestMessage::Start {
                idempotency_key: &start.idempotency_key,
                input: start.input.as_ref(),
                out_of_band_inputs: !start.durable_input_mappings.is_empty(),
            },
            Some(invocation_request::Request::ResumeAttach(resume)) => {
                RequestMessage::ResumeAttach {
                    idempotency_key: &resume.idempotency_key,
                    resume,
                }
            }
            Some(invocation_request::Request::InputItem(item)) => RequestMessage::InputItem(item),
            Some(invocation_request::Request::InputEnd(end)) => RequestMessage::InputEnd(end),
            Some(invocation_request::Request::StreamCancel(cancel)) => {
                RequestMessage::StreamCancel(cancel)
            }
            None => return Err("invocation request has no payload".to_string()),
        };
        self.validate_request(message, accept_terminal_output_cancellation)
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
                SessionPhase::AwaitDecision { .. },
                invocation_response::Response::Accepted(accepted),
            ) => self.accept(accepted),
            (
                SessionPhase::AwaitDecision { .. },
                invocation_response::Response::Rejected(rejected),
            ) => {
                let reason =
                    InvocationRejectionReason::try_from(rejected.reason).map_err(|_| {
                        format!("invalid invocation rejection reason {}", rejected.reason)
                    })?;
                if reason == InvocationRejectionReason::Unspecified {
                    return Err("invocation rejection reason is unspecified".to_string());
                }
                self.validate_idempotency_key(&rejected.idempotency_key)?;
                self.phase = SessionPhase::Complete;
                Ok(())
            }
            (SessionPhase::AwaitDecision { .. }, _) => {
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
                let durable_frame = self.validate_output_frame(
                    item.transport_stream_id,
                    &item.durable_stream_id,
                    &item.durable_offset,
                    item.epoch,
                )?;
                let value = item
                    .value
                    .as_ref()
                    .ok_or_else(|| "output stream item has no value".to_string())?;
                let state = self.outputs.get(&item.transport_stream_id).ok_or_else(|| {
                    format!("output stream {} is unknown", item.transport_stream_id)
                })?;
                ensure_open(state.terminal, item.transport_stream_id)?;
                let expected_sequence = if state.resume_first_frame {
                    item.producer_sequence
                } else {
                    state.next_offset
                };
                if item.producer_sequence != expected_sequence {
                    return Err(format!(
                        "output stream {} expected offset {}, got {}",
                        item.transport_stream_id, expected_sequence, item.producer_sequence
                    ));
                }
                let next_offset = expected_sequence.checked_add(1).ok_or_else(|| {
                    format!("output stream {} offset overflow", item.transport_stream_id)
                })?;
                let discovered = stream_references(value)?;
                let bindings = self.validate_new_mappings(
                    &item.new_stream_mappings,
                    &discovered,
                    StreamMappingRole::Output,
                )?;
                self.accept_output_stream_mappings(bindings)?;
                let state = self.outputs.get_mut(&item.transport_stream_id).unwrap();
                state.resume_first_frame = false;
                state.next_offset = next_offset;
                state.last_durable_offset = Some(durable_frame.durable_offset);
                Ok(())
            }
            (SessionPhase::Active, invocation_response::Response::OutputEnd(end)) => {
                let durable_frame = self.validate_output_frame(
                    end.transport_stream_id,
                    &end.durable_stream_id,
                    &end.durable_offset,
                    end.epoch,
                )?;
                self.terminate_output(
                    end.transport_stream_id,
                    end.producer_sequence,
                    durable_frame,
                )
            }
            (SessionPhase::Active, invocation_response::Response::OutputError(error)) => {
                let durable_frame = self.validate_output_frame(
                    error.transport_stream_id,
                    &error.durable_stream_id,
                    &error.durable_offset,
                    error.epoch,
                )?;
                self.terminate_output(
                    error.transport_stream_id,
                    error.producer_sequence,
                    durable_frame,
                )
            }
            (SessionPhase::Active, invocation_response::Response::InputAck(ack)) => {
                self.validate_ack(ack)
            }
            (SessionPhase::Active, invocation_response::Response::StreamCancel(cancel)) => {
                validate_cancel(cancel)?;
                match cancel.role() {
                    StreamCancelRole::InputConsumer => {
                        let durable_offset = self.validate_input_cancel_frame(cancel)?;
                        self.cancel_input(
                            cancel.transport_stream_id,
                            cancel.producer_sequence,
                            true,
                        )?;
                        self.inputs
                            .get_mut(&cancel.transport_stream_id)
                            .expect("input stream disappeared while applying cancellation")
                            .last_durable_offset = durable_offset;
                        Ok(())
                    }
                    StreamCancelRole::OutputProducer => {
                        let durable_frame = self.validate_output_frame(
                            cancel.transport_stream_id,
                            &cancel.durable_stream_id,
                            &cancel.durable_offset,
                            cancel.epoch,
                        )?;
                        self.confirm_output_cancellation(
                            cancel.transport_stream_id,
                            cancel.producer_sequence,
                            durable_frame,
                        )
                    }
                    _ => Err(
                        "server response may only cancel an input consumer or output producer"
                            .to_string(),
                    ),
                }
            }
            (SessionPhase::Active, invocation_response::Response::AttachmentRevoked(_)) => {
                self.phase = SessionPhase::Complete;
                Ok(())
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

    pub fn all_inputs_terminal(&self) -> bool {
        self.inputs.values().all(|state| state.terminal)
    }

    fn validate_request(
        &mut self,
        message: RequestMessage<'_>,
        accept_terminal_output_cancellation: bool,
    ) -> Result<(), String> {
        match (self.phase, message) {
            (
                SessionPhase::Initial,
                RequestMessage::Start {
                    idempotency_key,
                    input,
                    out_of_band_inputs,
                },
            ) => self.start(idempotency_key, input, false, out_of_band_inputs),
            (
                SessionPhase::Initial,
                RequestMessage::ResumeAttach {
                    idempotency_key,
                    resume,
                },
            ) => self.start_resume(idempotency_key, resume),
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
                self.validate_input_end(end.transport_stream_id, end.sequence)
            }
            (SessionPhase::Active, RequestMessage::StreamCancel(cancel)) => {
                validate_cancel(cancel)?;
                match cancel.role() {
                    StreamCancelRole::InputProducer => {
                        self.validate_cancel_request_authority(cancel, true)?;
                        self.cancel_input(
                            cancel.transport_stream_id,
                            cancel.producer_sequence,
                            false,
                        )
                    }
                    StreamCancelRole::OutputConsumer => {
                        self.validate_cancel_request_authority(cancel, false)?;
                        self.request_output_cancellation(
                            cancel.transport_stream_id,
                            cancel.producer_sequence,
                            accept_terminal_output_cancellation,
                        )
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
        out_of_band_inputs: bool,
    ) -> Result<(), String> {
        let key = required_idempotency_key(idempotency_key)?;
        let stream_ids = input
            .map(stream_references)
            .transpose()?
            .unwrap_or_default();
        self.inputs = stream_ids
            .into_iter()
            .map(|stream_id| {
                (
                    stream_id,
                    InputState {
                        terminal: out_of_band_inputs,
                        ..InputState::default()
                    },
                )
            })
            .collect();
        self.idempotency_key = Some(key.to_string());
        self.phase = SessionPhase::AwaitDecision { resume };
        Ok(())
    }

    fn start_resume(
        &mut self,
        idempotency_key: &Option<IdempotencyKey>,
        resume: &ResumeAttach,
    ) -> Result<(), String> {
        let key = required_idempotency_key(idempotency_key)?;
        let agent_id = resume
            .agent_id
            .as_ref()
            .ok_or_else(|| "resume-attach has no agent identity".to_string())?;
        validate_agent_id(agent_id, "resume-attach agent")?;
        let environment_id = resume
            .environment_id
            .as_ref()
            .ok_or_else(|| "resume-attach has no environment identity".to_string())?;
        required_uuid(&environment_id.value, "resume-attach environment ID")?;
        required_uuid(&resume.attachment_id, "resume-attach attachment ID")?;
        required_uuid(&resume.attempt_id, "resume-attach attempt ID")?;
        required_uuid(
            &resume.expected_callee_fingerprint,
            "resume-attach expected callee fingerprint",
        )?;
        if resume.expected_epoch == 0 {
            return Err("resume-attach expected epoch must be positive".to_string());
        }
        if matches!(resume.operation(), ResumeOperation::Unspecified) {
            return Err("resume-attach operation is unspecified".to_string());
        }
        let mut cursors = HashMap::with_capacity(resume.cursors.len());
        let mut previous = None;
        for cursor in &resume.cursors {
            let stream_id = required_uuid(&cursor.stream_id, "resume cursor stream ID")?;
            let stream_id = (stream_id.high_bits, stream_id.low_bits);
            if previous.is_some_and(|previous| previous >= stream_id) {
                return Err(
                    "resume cursors must be unique and sorted by durable stream ID".to_string(),
                );
            }
            previous = Some(stream_id);
            let offset = cursor
                .last_observed_offset
                .as_ref()
                .map(|offset| {
                    validate_durable_offset(offset)?;
                    Ok::<_, String>(
                        offset
                            .as_slice()
                            .try_into()
                            .expect("durable offset length was validated above"),
                    )
                })
                .transpose()?;
            cursors.insert(stream_id, offset);
        }
        self.idempotency_key = Some(key.to_string());
        self.resume = true;
        self.resume_cursors = cursors;
        self.resume_agent_id = resume.agent_id.clone();
        self.resume_environment_id = resume.environment_id;
        self.resume_attachment_id = resume.attachment_id;
        self.resume_attempt_id = resume.attempt_id;
        self.resume_callee_fingerprint = resume.expected_callee_fingerprint;
        self.resume_accepted_epoch = Some(
            resume
                .expected_epoch
                .checked_add(1)
                .ok_or_else(|| "resume-attach epoch cannot wrap".to_string())?,
        );
        self.phase = SessionPhase::AwaitDecision { resume: true };
        Ok(())
    }

    fn accept(&mut self, accepted: &InvocationAccepted) -> Result<(), String> {
        self.validate_idempotency_key(&accepted.idempotency_key)?;
        let agent_id = accepted
            .agent_id
            .as_ref()
            .ok_or_else(|| "invocation acceptance has no agent identity".to_string())?;
        if agent_id.name.is_empty() {
            return Err("invocation acceptance has an empty agent name".to_string());
        }
        if self.resume {
            required_uuid(
                &accepted.attachment_id,
                "invocation acceptance attachment ID",
            )?;
            required_uuid(&accepted.attempt_id, "invocation acceptance attempt ID")?;
            if accepted.epoch == 0 {
                return Err("invocation acceptance epoch must be positive".to_string());
            }
            if accepted.agent_id != self.resume_agent_id
                || accepted.environment_id != self.resume_environment_id
                || accepted.attachment_id != self.resume_attachment_id
                || accepted.attempt_id != self.resume_attempt_id
                || accepted.callee_fingerprint != self.resume_callee_fingerprint
                || Some(accepted.epoch) != self.resume_accepted_epoch
            {
                return Err(
                    "resumed invocation acceptance does not match the requested attachment identity"
                        .to_string(),
                );
            }
            accepted.component_revision.ok_or_else(|| {
                "durable invocation acceptance has no pinned component revision".to_string()
            })?;
            validate_accepted_mappings(&accepted.stream_mappings)?;
            let mapped_stream_ids = accepted
                .stream_mappings
                .iter()
                .map(mapping_stream_id)
                .collect::<Result<HashSet<_>, _>>()?;
            if self
                .resume_cursors
                .keys()
                .any(|stream_id| !mapped_stream_ids.contains(stream_id))
            {
                return Err("resume cursor names a stream absent from acceptance".to_string());
            }
            let mapped_output_stream_ids = accepted
                .stream_mappings
                .iter()
                .filter(|mapping| mapping.role() == StreamMappingRole::Output)
                .map(mapping_stream_id)
                .collect::<Result<HashSet<_>, _>>()?;
            if self
                .resume_cursors
                .keys()
                .any(|stream_id| !mapped_output_stream_ids.contains(stream_id))
            {
                return Err("resume cursor may only name an output stream".to_string());
            }
            for mapping in &accepted.stream_mappings {
                let durable_stream_id = mapping_stream_id(mapping)?;
                match mapping.role() {
                    StreamMappingRole::Input => {
                        let (next_offset, last_durable_offset, terminal) = mapping
                            .high_water
                            .as_ref()
                            .map(|high_water| {
                                let next = high_water
                                    .highest_contiguous_sequence
                                    .checked_add(1)
                                    .ok_or_else(|| {
                                        "input high-water sequence overflow".to_string()
                                    })?;
                                let offset = high_water
                                    .resulting_offset
                                    .as_slice()
                                    .try_into()
                                    .expect("durable offset length was validated above");
                                Ok::<_, String>((next, Some(offset), high_water.terminal))
                            })
                            .transpose()?
                            .unwrap_or((0, None, false));
                        self.inputs.insert(
                            mapping.transport_stream_id,
                            InputState {
                                next_offset,
                                terminal,
                                durable_stream_id: Some(durable_stream_id),
                                last_durable_offset,
                                ..InputState::default()
                            },
                        );
                    }
                    StreamMappingRole::Output => {
                        self.outputs.insert(
                            mapping.transport_stream_id,
                            OutputState {
                                resume_first_frame: true,
                                resume_mapping_announcement_pending: true,
                                durable_stream_id,
                                last_durable_offset: self
                                    .resume_cursors
                                    .get(&durable_stream_id)
                                    .copied()
                                    .flatten(),
                                ..OutputState::default()
                            },
                        );
                    }
                    StreamMappingRole::Unspecified => unreachable!(
                        "accepted mapping roles were validated before resume initialization"
                    ),
                }
            }
            self.accepted_agent_id = Some(agent_id.clone());
            self.accepted_revision = accepted.component_revision;
            self.accepted_epoch = Some(accepted.epoch);
            self.phase = SessionPhase::Active;
            return Ok(());
        }
        let durable_acceptance = !self.inputs.is_empty()
            || accepted.attachment_id.is_some()
            || accepted.attempt_id.is_some()
            || accepted.epoch != 0
            || !accepted.stream_mappings.is_empty()
            || accepted.environment_id.is_some()
            || accepted.callee_fingerprint.is_some();
        if durable_acceptance {
            required_uuid(
                &accepted.attachment_id,
                "invocation acceptance attachment ID",
            )?;
            required_uuid(&accepted.attempt_id, "invocation acceptance attempt ID")?;
            if accepted.epoch == 0 {
                return Err("invocation acceptance epoch must be positive".to_string());
            }
            required_uuid(
                &accepted
                    .environment_id
                    .as_ref()
                    .ok_or_else(|| {
                        "durable invocation acceptance has no environment identity".to_string()
                    })?
                    .value,
                "durable invocation acceptance environment ID",
            )?;
            required_uuid(
                &accepted.callee_fingerprint,
                "durable invocation acceptance callee fingerprint",
            )?;
            accepted.component_revision.ok_or_else(|| {
                "durable invocation acceptance has no pinned component revision".to_string()
            })?;
            validate_accepted_mappings(&accepted.stream_mappings)?;
            let expected_inputs = self.inputs.keys().copied().collect::<HashSet<_>>();
            let mapped_inputs = accepted
                .stream_mappings
                .iter()
                .filter(|mapping| mapping.role() == StreamMappingRole::Input)
                .map(|mapping| mapping.transport_stream_id)
                .collect::<HashSet<_>>();
            if mapped_inputs != expected_inputs
                || accepted
                    .stream_mappings
                    .iter()
                    .any(|mapping| mapping.role() != StreamMappingRole::Input)
            {
                return Err(
                    "fresh invocation acceptance mappings do not match its initial inputs"
                        .to_string(),
                );
            }
            let bindings = accepted
                .stream_mappings
                .iter()
                .map(|mapping| {
                    let durable_stream_id = mapping_stream_id(mapping)?;
                    let high_water = mapping
                        .high_water
                        .as_ref()
                        .map(|high_water| {
                            let next_offset = high_water
                                .highest_contiguous_sequence
                                .checked_add(1)
                                .ok_or_else(|| "input high-water sequence overflow".to_string())?;
                            let durable_offset = high_water
                                .resulting_offset
                                .as_slice()
                                .try_into()
                                .expect("durable offset length was validated above");
                            Ok::<_, String>((next_offset, durable_offset, high_water.terminal))
                        })
                        .transpose()?;
                    Ok((mapping.transport_stream_id, durable_stream_id, high_water))
                })
                .collect::<Result<Vec<_>, String>>()?;
            for (transport_stream_id, durable_stream_id, high_water) in bindings {
                let state = self
                    .inputs
                    .get_mut(&transport_stream_id)
                    .expect("accepted input mapping set was validated above");
                state.durable_stream_id = Some(durable_stream_id);
                if let Some((next_offset, durable_offset, terminal)) = high_water {
                    state.next_offset = next_offset;
                    state.last_durable_offset = Some(durable_offset);
                    state.terminal = terminal;
                }
            }
        }
        self.accepted_agent_id = Some(agent_id.clone());
        self.accepted_revision = accepted.component_revision;
        self.accepted_epoch = durable_acceptance.then_some(accepted.epoch);
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
        let bindings = self.validate_new_mappings(
            &result.new_stream_mappings,
            &discovered,
            StreamMappingRole::Output,
        )?;
        self.accept_output_stream_mappings(bindings)?;
        self.has_result = true;
        Ok(())
    }

    fn validate_input_item(&mut self, item: &InputStreamItem) -> Result<(), String> {
        let payload = item
            .payload
            .as_ref()
            .ok_or_else(|| "input stream item has no payload".to_string())?;
        let (logical_item_count, discovered) = match payload {
            Payload::Value(value) => (1, stream_references(value)?),
            Payload::PackedU8(bytes) if !bytes.is_empty() => (bytes.len() as u64, Vec::new()),
            Payload::PackedU8(_) => {
                return Err("packed-u8 input item must not be empty".to_string());
            }
        };
        let identity = InputItemIdentity {
            sequence: item.sequence,
            logical_item_count,
            new_stream_ids: discovered.clone(),
            payload_fingerprint: input_payload_fingerprint(payload),
            terminal: false,
        };
        let item_end = item
            .sequence
            .checked_add(logical_item_count)
            .ok_or_else(|| format!("input stream {} offset overflow", item.transport_stream_id))?;
        let state = self
            .inputs
            .get(&item.transport_stream_id)
            .ok_or_else(|| format!("input stream {} is unknown", item.transport_stream_id))?;
        let current_offset = state.next_offset;
        let overlapping = state
            .pending_ranges
            .values()
            .find(|pending| {
                let pending_end = pending
                    .item
                    .sequence
                    .checked_add(pending.item.logical_item_count)
                    .expect("validated pending input range cannot overflow");
                item.sequence < pending_end && pending.item.sequence < item_end
            })
            .map(|pending| pending.item.sequence);
        if let Some(pending_sequence) = overlapping {
            let state = self
                .inputs
                .get_mut(&item.transport_stream_id)
                .expect("input stream was validated above");
            let pending = state
                .pending_ranges
                .get_mut(&pending_sequence)
                .expect("overlapping pending input range was found above");
            if pending.item != identity {
                return Err(format!(
                    "input stream {} range {}..{} conflicts with pending range {}..{}",
                    item.transport_stream_id,
                    item.sequence,
                    item_end,
                    pending.item.sequence,
                    pending.item.sequence + pending.item.logical_item_count
                ));
            }
            pending.outstanding_acks = pending
                .outstanding_acks
                .checked_add(1)
                .ok_or_else(|| "input acknowledgement count overflow".to_string())?;
            state.pending_acks.push_back(item.sequence);
            return Ok(());
        }
        if item.sequence < current_offset {
            if item_end > current_offset {
                return Err(format!(
                    "input stream {} retry range {}..{} overlaps its committed high-water {}",
                    item.transport_stream_id, item.sequence, item_end, current_offset
                ));
            }
            for stream_id in &discovered {
                if self.outputs.contains_key(stream_id) {
                    return Err(format!(
                        "stream {stream_id} is already registered as output"
                    ));
                }
            }
            let new_stream_ids = discovered
                .iter()
                .copied()
                .filter(|stream_id| !self.inputs.contains_key(stream_id))
                .collect::<Vec<_>>();
            self.insert_input_streams(new_stream_ids);
            self.inputs
                .get_mut(&item.transport_stream_id)
                .expect("input stream was validated above")
                .pending_ranges
                .insert(
                    item.sequence,
                    PendingInputRange {
                        item: identity,
                        historical: true,
                        durable_offset: None,
                        outstanding_acks: 1,
                    },
                );
            self.inputs
                .get_mut(&item.transport_stream_id)
                .expect("input stream was validated above")
                .pending_acks
                .push_back(item.sequence);
            return Ok(());
        }
        self.ensure_new_input_streams(&discovered)?;
        let state = self
            .inputs
            .get_mut(&item.transport_stream_id)
            .ok_or_else(|| format!("input stream {} is unknown", item.transport_stream_id))?;
        if let Some(discard_next_offset) = state.discard_next_offset.as_mut() {
            if item.sequence != *discard_next_offset {
                return Err(format!(
                    "cancelled input stream {} expected discarded sequence {}, got {}",
                    item.transport_stream_id, *discard_next_offset, item.sequence
                ));
            }
            *discard_next_offset = discard_next_offset
                .checked_add(logical_item_count)
                .ok_or_else(|| {
                    format!("input stream {} offset overflow", item.transport_stream_id)
                })?;
            self.insert_input_streams(discovered);
            return Ok(());
        }
        ensure_open(state.terminal, item.transport_stream_id)?;
        if item.sequence != state.next_offset {
            return Err(format!(
                "input stream {} expected sequence {}, got {}",
                item.transport_stream_id, state.next_offset, item.sequence
            ));
        }
        state.pending_ranges.insert(
            item.sequence,
            PendingInputRange {
                item: identity,
                historical: false,
                durable_offset: None,
                outstanding_acks: 1,
            },
        );
        state.pending_acks.push_back(item.sequence);
        state.next_offset = item_end;
        self.insert_input_streams(discovered);
        Ok(())
    }

    fn validate_ack(&mut self, ack: &InputStreamAck) -> Result<(), String> {
        let expected_epoch = self
            .accepted_epoch
            .ok_or_else(|| "input acknowledgement has no durable acceptance".to_string())?;
        if ack.epoch < expected_epoch {
            return Err("input acknowledgement uses a stale attachment epoch".to_string());
        }
        if ack.epoch > expected_epoch {
            return Err("input acknowledgement uses a future attachment epoch".to_string());
        }
        let durable_stream_id = required_uuid(
            &ack.durable_stream_id,
            "input acknowledgement durable stream ID",
        )?;
        let durable_stream_id = (durable_stream_id.high_bits, durable_stream_id.low_bits);
        validate_durable_offset(&ack.resulting_offset)?;
        let durable_offset: [u8; 24] = ack
            .resulting_offset
            .as_slice()
            .try_into()
            .expect("durable offset length was validated above");
        let state = self
            .inputs
            .get(&ack.transport_stream_id)
            .ok_or_else(|| format!("input stream {} is unknown", ack.transport_stream_id))?;
        if state.durable_stream_id != Some(durable_stream_id) {
            return Err(format!(
                "input stream {} acknowledgement identity differs from its announced mapping",
                ack.transport_stream_id
            ));
        }
        let expected_sequence = state.pending_acks.front().copied().ok_or_else(|| {
            format!(
                "input stream {} has no item awaiting acknowledgement",
                ack.transport_stream_id
            )
        })?;
        let expected = state
            .pending_ranges
            .get(&expected_sequence)
            .cloned()
            .expect("every pending acknowledgement has a pending range");
        if let Some(expected_offset) = expected.durable_offset {
            if durable_offset != expected_offset {
                return Err(format!(
                    "input stream {} retry acknowledgement changed its durable offset",
                    ack.transport_stream_id
                ));
            }
        } else if expected.historical {
            let invalid_historical_offset = if expected.item.terminal {
                state.last_durable_offset != Some(durable_offset)
            } else {
                state
                    .last_durable_offset
                    .is_none_or(|high_water| durable_offset > high_water)
            };
            if invalid_historical_offset {
                return Err(format!(
                    "input stream {} historical acknowledgement does not match its durable high-water",
                    ack.transport_stream_id
                ));
            }
        } else if state
            .last_durable_offset
            .is_some_and(|previous| durable_offset <= previous)
        {
            return Err(format!(
                "input stream {} acknowledgement offset did not strictly increase",
                ack.transport_stream_id
            ));
        }
        let expected_highest_sequence = expected
            .item
            .sequence
            .checked_add(expected.item.logical_item_count - 1)
            .ok_or_else(|| "input acknowledgement sequence overflow".to_string())?;
        if ack.highest_contiguous_sequence != expected_highest_sequence
            || ack.logical_item_count != expected.item.logical_item_count
        {
            return Err(format!(
                "input stream {} expected acknowledgement ({}, {}), got ({}, {})",
                ack.transport_stream_id,
                expected_highest_sequence,
                expected.item.logical_item_count,
                ack.highest_contiguous_sequence,
                ack.logical_item_count
            ));
        }
        let bindings = self.validate_new_mappings(
            &ack.new_stream_mappings,
            &expected.item.new_stream_ids,
            StreamMappingRole::Input,
        )?;
        for (transport_stream_id, durable_stream_id) in &bindings {
            let state = self
                .inputs
                .get(transport_stream_id)
                .expect("nested input stream was discovered before its acknowledgement");
            if state
                .durable_stream_id
                .is_some_and(|expected| expected != *durable_stream_id)
            {
                return Err(format!(
                    "input stream {transport_stream_id} acknowledgement mapping changes its durable identity"
                ));
            }
        }
        for (transport_stream_id, durable_stream_id) in bindings {
            self.inputs
                .get_mut(&transport_stream_id)
                .expect("nested input stream was discovered before its acknowledgement")
                .durable_stream_id = Some(durable_stream_id);
        }
        let state = self
            .inputs
            .get_mut(&ack.transport_stream_id)
            .expect("input stream was validated above");
        if expected.durable_offset.is_none() && !expected.historical {
            state.last_durable_offset = Some(durable_offset);
        }
        let pending = state
            .pending_ranges
            .get_mut(&expected_sequence)
            .expect("pending range was validated above");
        pending.durable_offset = Some(durable_offset);
        pending.outstanding_acks = pending
            .outstanding_acks
            .checked_sub(1)
            .expect("pending range has an acknowledgement expectation");
        let range_complete = pending.outstanding_acks == 0;
        state.pending_acks.pop_front();
        if range_complete && !expected.item.terminal {
            state.pending_ranges.remove(&expected_sequence);
        }
        Ok(())
    }

    fn validate_input_end(&mut self, stream_id: u64, offset: u64) -> Result<(), String> {
        let identity = InputItemIdentity {
            sequence: offset,
            logical_item_count: 1,
            new_stream_ids: Vec::new(),
            payload_fingerprint: [0; 32],
            terminal: true,
        };
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

        if let Some(pending) = state.pending_ranges.get_mut(&offset) {
            if pending.item != identity {
                return Err(format!(
                    "input stream {stream_id} terminal conflicts with a pending input event at sequence {offset}"
                ));
            }
            pending.outstanding_acks = pending
                .outstanding_acks
                .checked_add(1)
                .ok_or_else(|| "input acknowledgement count overflow".to_string())?;
            state.pending_acks.push_back(offset);
            return Ok(());
        }

        ensure_open(state.terminal, stream_id)?;
        let historical = if offset == state.next_offset {
            false
        } else if offset
            .checked_add(1)
            .is_some_and(|next_offset| next_offset == state.next_offset)
        {
            true
        } else {
            return Err(format!(
                "input stream {stream_id} expected terminal offset {}, got {offset}",
                state.next_offset
            ));
        };

        state.pending_ranges.insert(
            offset,
            PendingInputRange {
                item: identity,
                historical,
                durable_offset: None,
                outstanding_acks: 1,
            },
        );
        state.pending_acks.push_back(offset);
        if !historical {
            state.next_offset = state
                .next_offset
                .checked_add(1)
                .ok_or_else(|| format!("input stream {stream_id} offset overflow"))?;
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
            .copied()
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
        state.pending_ranges.clear();
        state.pending_acks.clear();
        state.terminal = true;
        Ok(())
    }

    fn terminate_output(
        &mut self,
        stream_id: u64,
        offset: u64,
        durable_frame: ValidatedOutputFrame,
    ) -> Result<(), String> {
        let state = self
            .outputs
            .get_mut(&stream_id)
            .ok_or_else(|| format!("output stream {stream_id} is unknown"))?;
        ensure_open(state.terminal, stream_id)?;
        if state.resume_first_frame {
            state.next_offset = offset;
            state.resume_first_frame = false;
        }
        if offset != state.next_offset {
            return Err(format!(
                "output stream {stream_id} expected terminal offset {}, got {offset}",
                state.next_offset
            ));
        }
        state.terminal = true;
        state.last_durable_offset = Some(durable_frame.durable_offset);
        Ok(())
    }

    fn request_output_cancellation(
        &mut self,
        stream_id: u64,
        offset: u64,
        accept_terminal: bool,
    ) -> Result<(), String> {
        let state = self
            .outputs
            .get_mut(&stream_id)
            .ok_or_else(|| format!("output stream {stream_id} is unknown"))?;
        if !accept_terminal {
            ensure_open(state.terminal, stream_id)?;
        }
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

    fn confirm_output_cancellation(
        &mut self,
        stream_id: u64,
        offset: u64,
        durable_frame: ValidatedOutputFrame,
    ) -> Result<(), String> {
        let requested = {
            let state = self
                .outputs
                .get(&stream_id)
                .ok_or_else(|| format!("output stream {stream_id} is unknown"))?;
            ensure_open(state.terminal, stream_id)?;
            state.cancellation_requested
        };
        match requested {
            Some(requested_offset) if offset < requested_offset => Err(format!(
                "output stream {stream_id} cancellation confirmation precedes the requested offset {requested_offset}: got {offset}"
            )),
            Some(_) => {
                let state = self
                    .outputs
                    .get_mut(&stream_id)
                    .expect("output stream disappeared while confirming cancellation");
                if offset != state.next_offset {
                    return Err(format!(
                        "output stream {stream_id} expected cancellation terminal at {}, got {offset}",
                        state.next_offset
                    ));
                }
                state.terminal = true;
                state.last_durable_offset = Some(durable_frame.durable_offset);
                Ok(())
            }
            None => {
                let state = self
                    .outputs
                    .get_mut(&stream_id)
                    .ok_or_else(|| format!("output stream {stream_id} is unknown"))?;
                ensure_open(state.terminal, stream_id)?;
                if offset != state.next_offset {
                    return Err(format!(
                        "output stream {stream_id} expected cancellation offset {}, got {offset}",
                        state.next_offset
                    ));
                }
                state.terminal = true;
                state.last_durable_offset = Some(durable_frame.durable_offset);
                Ok(())
            }
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

    fn accept_output_stream_mappings(
        &mut self,
        mappings: Vec<(u64, (u64, u64))>,
    ) -> Result<(), String> {
        for (stream_id, durable_stream_id) in &mappings {
            if self.inputs.contains_key(stream_id) {
                return Err(format!("stream {stream_id} is already registered"));
            }
            if let Some(state) = self.outputs.get(stream_id) {
                if !state.resume_mapping_announcement_pending {
                    return Err(format!("stream {stream_id} is already registered"));
                }
                if state.durable_stream_id != *durable_stream_id {
                    return Err(format!(
                        "resumed output stream {stream_id} durable identity differs from its acceptance mapping"
                    ));
                }
            }
        }
        for (stream_id, durable_stream_id) in mappings {
            if let Some(state) = self.outputs.get_mut(&stream_id) {
                state.resume_mapping_announcement_pending = false;
            } else {
                self.outputs.insert(
                    stream_id,
                    OutputState {
                        durable_stream_id,
                        ..OutputState::default()
                    },
                );
            }
        }
        Ok(())
    }

    fn validate_new_mappings(
        &self,
        mappings: &[DurableStreamMapping],
        expected_transport_stream_ids: &[u64],
        expected_role: StreamMappingRole,
    ) -> Result<Vec<StreamBinding>, String> {
        validate_accepted_mappings(mappings)?;
        let actual = mappings
            .iter()
            .map(|mapping| {
                let role = StreamMappingRole::try_from(mapping.role)
                    .map_err(|_| format!("invalid durable stream mapping role {}", mapping.role))?;
                if role != expected_role {
                    return Err(format!(
                        "new stream mapping has role {role:?}, expected {expected_role:?}"
                    ));
                }
                if mapping.high_water.is_some() {
                    return Err("new stream mapping cannot carry high-water state".to_string());
                }
                Ok(mapping.transport_stream_id)
            })
            .collect::<Result<HashSet<_>, String>>()?;
        let expected = expected_transport_stream_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if actual != expected {
            return Err("new durable stream mappings do not match the recursive value".to_string());
        }
        mappings
            .iter()
            .map(|mapping| Ok((mapping.transport_stream_id, mapping_stream_id(mapping)?)))
            .collect()
    }

    fn validate_output_frame(
        &self,
        transport_stream_id: u64,
        durable_stream_id: &Option<crate::proto::golem::common::Uuid>,
        durable_offset: &[u8],
        epoch: u64,
    ) -> Result<ValidatedOutputFrame, String> {
        let durable_stream_id = required_uuid(durable_stream_id, "durable output frame stream ID")?;
        validate_durable_offset(durable_offset)?;
        let expected_epoch = self
            .accepted_epoch
            .ok_or_else(|| "durable output frame follows a non-durable acceptance".to_string())?;
        if epoch < expected_epoch {
            return Err("durable output frame uses a stale attachment epoch".to_string());
        }
        if epoch > expected_epoch {
            return Err("durable output frame uses a future attachment epoch".to_string());
        }
        let state = self
            .outputs
            .get(&transport_stream_id)
            .ok_or_else(|| format!("output stream {transport_stream_id} is unknown"))?;
        let durable_stream_id = (durable_stream_id.high_bits, durable_stream_id.low_bits);
        if state.durable_stream_id != durable_stream_id {
            return Err(format!(
                "output stream {transport_stream_id} durable stream identity differs from its announced mapping"
            ));
        }
        let durable_offset: [u8; 24] = durable_offset
            .try_into()
            .expect("durable offset length was validated above");
        if state
            .last_durable_offset
            .is_some_and(|previous| durable_offset <= previous)
        {
            return Err(format!(
                "output stream {transport_stream_id} durable offset did not strictly increase"
            ));
        }
        Ok(ValidatedOutputFrame { durable_offset })
    }

    fn validate_input_cancel_frame(
        &self,
        cancel: &StreamCancel,
    ) -> Result<Option<[u8; 24]>, String> {
        let state = self
            .inputs
            .get(&cancel.transport_stream_id)
            .ok_or_else(|| format!("input stream {} is unknown", cancel.transport_stream_id))?;
        let Some(expected_stream_id) = state.durable_stream_id else {
            if cancel.durable_stream_id.is_some()
                || !cancel.durable_offset.is_empty()
                || cancel.epoch != 0
            {
                return Err(
                    "non-durable input cancellation contains durable stream metadata".to_string(),
                );
            }
            return Ok(None);
        };
        let durable_stream_id = required_uuid(
            &cancel.durable_stream_id,
            "durable input cancellation stream ID",
        )?;
        if (durable_stream_id.high_bits, durable_stream_id.low_bits) != expected_stream_id {
            return Err(format!(
                "input stream {} durable stream identity differs from its announced mapping",
                cancel.transport_stream_id
            ));
        }
        validate_durable_offset(&cancel.durable_offset)?;
        let expected_epoch = self.accepted_epoch.ok_or_else(|| {
            "durable input cancellation follows a non-durable acceptance".to_string()
        })?;
        if cancel.epoch < expected_epoch {
            return Err("durable input cancellation uses a stale attachment epoch".to_string());
        }
        if cancel.epoch > expected_epoch {
            return Err("durable input cancellation uses a future attachment epoch".to_string());
        }
        let durable_offset: [u8; 24] = cancel
            .durable_offset
            .as_slice()
            .try_into()
            .expect("durable offset length was validated above");
        if state
            .last_durable_offset
            .is_some_and(|previous| durable_offset <= previous)
        {
            return Err(format!(
                "input stream {} cancellation durable offset did not strictly increase",
                cancel.transport_stream_id
            ));
        }
        Ok(Some(durable_offset))
    }

    fn validate_cancel_request_authority(
        &self,
        cancel: &StreamCancel,
        input: bool,
    ) -> Result<(), String> {
        let expected_stream_id = if input {
            self.inputs
                .get(&cancel.transport_stream_id)
                .and_then(|state| state.durable_stream_id)
        } else {
            self.outputs
                .get(&cancel.transport_stream_id)
                .map(|state| state.durable_stream_id)
        };
        let Some(expected_stream_id) = expected_stream_id else {
            if cancel.durable_stream_id.is_some() || cancel.epoch != 0 {
                return Err(
                    "non-durable stream cancellation contains durable attachment metadata"
                        .to_string(),
                );
            }
            return Ok(());
        };
        let durable_stream_id = required_uuid(
            &cancel.durable_stream_id,
            "durable stream cancellation stream ID",
        )?;
        if (durable_stream_id.high_bits, durable_stream_id.low_bits) != expected_stream_id {
            return Err("durable stream cancellation names the wrong stream identity".to_string());
        }
        let expected_epoch = self
            .accepted_epoch
            .ok_or_else(|| "durable cancellation follows a non-durable acceptance".to_string())?;
        if cancel.epoch < expected_epoch {
            return Err("durable stream cancellation uses a stale attachment epoch".to_string());
        }
        if cancel.epoch > expected_epoch {
            return Err("durable stream cancellation uses a future attachment epoch".to_string());
        }
        if !cancel.durable_offset.is_empty() {
            return Err(
                "consumer cancellation request cannot claim a producer durable terminal offset"
                    .to_string(),
            );
        }
        Ok(())
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

fn required_uuid<'a>(
    value: &'a Option<crate::proto::golem::common::Uuid>,
    field: &str,
) -> Result<&'a crate::proto::golem::common::Uuid, String> {
    let value = value
        .as_ref()
        .ok_or_else(|| format!("{field} is missing"))?;
    if value.high_bits == 0 && value.low_bits == 0 {
        Err(format!("{field} must not be nil"))
    } else {
        Ok(value)
    }
}

fn validate_agent_id(agent_id: &AgentId, field: &str) -> Result<(), String> {
    if agent_id.name.is_empty() {
        return Err(format!("{field} has an empty agent name"));
    }
    let component_id = agent_id
        .component_id
        .as_ref()
        .ok_or_else(|| format!("{field} has no component ID"))?;
    required_uuid(&component_id.value, &format!("{field} component ID"))?;
    Ok(())
}

fn validate_accepted_mappings(mappings: &[DurableStreamMapping]) -> Result<(), String> {
    let mut transport_stream_ids = HashSet::with_capacity(mappings.len());
    let mut durable_stream_ids = HashSet::with_capacity(mappings.len());
    for mapping in mappings {
        if !transport_stream_ids.insert(mapping.transport_stream_id) {
            return Err(format!(
                "invocation acceptance repeats transport stream {}",
                mapping.transport_stream_id
            ));
        }
        let role = StreamMappingRole::try_from(mapping.role)
            .map_err(|_| format!("invalid durable stream mapping role {}", mapping.role))?;
        if role == StreamMappingRole::Unspecified {
            return Err("durable stream mapping role is unspecified".to_string());
        }
        if role == StreamMappingRole::Output && mapping.high_water.is_some() {
            return Err("output stream mapping cannot carry input high-water state".to_string());
        }
        if let Some(high_water) = &mapping.high_water {
            validate_durable_offset(&high_water.resulting_offset)?;
        }
        let handle = mapping
            .handle
            .as_ref()
            .ok_or_else(|| "durable stream mapping has no handle".to_string())?;
        validate_accepted_handle(handle)?;
        let stream_id = required_uuid(&handle.stream_id, "durable stream handle stream ID")?;
        if !durable_stream_ids.insert((stream_id.high_bits, stream_id.low_bits, role as i32)) {
            return Err(
                "invocation acceptance maps one durable stream more than once for one role"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn mapping_stream_id(mapping: &DurableStreamMapping) -> Result<(u64, u64), String> {
    let handle = mapping
        .handle
        .as_ref()
        .ok_or_else(|| "durable stream mapping has no handle".to_string())?;
    let stream_id = required_uuid(&handle.stream_id, "durable stream handle stream ID")?;
    Ok((stream_id.high_bits, stream_id.low_bits))
}

fn validate_accepted_handle(handle: &DurableStreamHandle) -> Result<(), String> {
    if handle.format_version != 1 {
        return Err(format!(
            "unsupported durable stream handle format version {}",
            handle.format_version
        ));
    }
    required_uuid(&handle.stream_id, "durable stream handle stream ID")?;
    let producer_environment = handle
        .producer_environment_id
        .as_ref()
        .ok_or_else(|| "durable stream handle has no producer environment".to_string())?;
    required_uuid(
        &producer_environment.value,
        "durable stream handle producer environment ID",
    )?;
    let producer = handle
        .producer
        .as_ref()
        .ok_or_else(|| "durable stream handle has no producer identity".to_string())?;
    validate_agent_id(producer, "durable stream producer")?;
    required_uuid(
        &handle.expected_producer_fingerprint,
        "durable stream handle expected producer fingerprint",
    )?;
    let source = handle
        .source_invocation
        .as_ref()
        .ok_or_else(|| "durable stream handle has no source invocation".to_string())?;
    let callee_environment = source
        .callee_environment_id
        .as_ref()
        .ok_or_else(|| "durable stream source has no callee environment".to_string())?;
    required_uuid(
        &callee_environment.value,
        "durable stream source callee environment ID",
    )?;
    let source_callee = source
        .callee
        .as_ref()
        .ok_or_else(|| "durable stream source has no callee identity".to_string())?;
    validate_agent_id(source_callee, "durable stream source callee")?;
    required_uuid(
        &source.callee_fingerprint,
        "durable stream source callee fingerprint",
    )?;
    required_idempotency_key(&source.idempotency_key)?;
    handle
        .component_revision
        .ok_or_else(|| "durable stream handle has no pinned component revision".to_string())?;
    if handle.element_schema_fingerprint.len() != 32 {
        return Err("durable stream handle schema fingerprint must contain 32 bytes".to_string());
    }
    Ok(())
}

fn validate_durable_offset(offset: &[u8]) -> Result<(), String> {
    if offset.len() != 24
        || offset[0] != 1
        || offset[1..8].iter().any(|byte| *byte != 0)
        || offset[20..24].iter().any(|byte| *byte != 0)
    {
        return Err("input high-water contains an invalid durable stream offset".to_string());
    }
    Ok(())
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

fn input_payload_fingerprint(payload: &Payload) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    match payload {
        Payload::Value(value) => {
            hasher.update(&[0]);
            hasher.update(&value.encode_to_vec());
        }
        Payload::PackedU8(bytes) => {
            hasher.update(&[1]);
            hasher.update(bytes);
        }
    }
    *hasher.finalize().as_bytes()
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
    use crate::proto::golem::common::{Empty, EnvironmentId, Uuid};
    use crate::proto::golem::component::ComponentId;
    use crate::proto::golem::schema::{RecordValue, SchemaValueStreamReference};
    use crate::proto::golem::worker::{
        InputStreamHighWater, InvocationFailure, InvocationRejected, InvocationStart,
        OutputStreamEnd, OutputStreamError, OutputStreamItem, PublicInvocationStart, ResumeAttach,
        StreamCursor, StreamInvocationIdentity,
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
            component_id: Some(ComponentId {
                value: Some(uuid(20)),
            }),
            name: "agent".to_string(),
        }
    }

    fn uuid(value: u64) -> Uuid {
        Uuid {
            high_bits: 0,
            low_bits: value,
        }
    }

    fn accepted() -> InvocationResponse {
        response(invocation_response::Response::Accepted(
            InvocationAccepted {
                agent_id: Some(agent_id()),
                idempotency_key: key(),
                component_revision: Some(12),
                attachment_id: Some(uuid(1)),
                attempt_id: Some(uuid(2)),
                epoch: 1,
                environment_id: Some(EnvironmentId {
                    value: Some(uuid(3)),
                }),
                callee_fingerprint: Some(uuid(4)),
                ..Default::default()
            },
        ))
    }

    fn accepted_with_inputs(stream_ids: &[u64]) -> InvocationResponse {
        let mut acceptance = accepted();
        let Some(invocation_response::Response::Accepted(accepted)) = acceptance.response.as_mut()
        else {
            unreachable!("accepted helper returned the wrong response")
        };
        accepted.stream_mappings = stream_ids
            .iter()
            .map(|stream_id| mapping(*stream_id, StreamMappingRole::Input))
            .collect();
        acceptance
    }

    fn resume_attach(cursors: Vec<StreamCursor>) -> PublicInvocationRequest {
        public_request(public_invocation_request::Request::ResumeAttach(
            ResumeAttach {
                idempotency_key: key(),
                agent_id: Some(agent_id()),
                environment_id: Some(EnvironmentId {
                    value: Some(uuid(3)),
                }),
                attachment_id: Some(uuid(1)),
                attempt_id: Some(uuid(5)),
                expected_callee_fingerprint: Some(uuid(4)),
                expected_epoch: 1,
                operation: ResumeOperation::Resume as i32,
                cursors,
                ..Default::default()
            },
        ))
    }

    fn resumed_acceptance(mappings: Vec<DurableStreamMapping>) -> InvocationResponse {
        let mut acceptance = accepted();
        let Some(invocation_response::Response::Accepted(accepted)) = acceptance.response.as_mut()
        else {
            unreachable!("accepted helper returned the wrong response")
        };
        accepted.attempt_id = Some(uuid(5));
        accepted.epoch = 2;
        accepted.stream_mappings = mappings;
        acceptance
    }

    fn input_ack(
        transport_stream_id: u64,
        sequence: u64,
        resulting_offset: Vec<u8>,
    ) -> InvocationResponse {
        response(invocation_response::Response::InputAck(InputStreamAck {
            transport_stream_id,
            highest_contiguous_sequence: sequence,
            logical_item_count: 1,
            durable_stream_id: Some(uuid(100 + transport_stream_id)),
            resulting_offset,
            epoch: 1,
            new_stream_mappings: Vec::new(),
        }))
    }

    fn mapping(transport_stream_id: u64, role: StreamMappingRole) -> DurableStreamMapping {
        DurableStreamMapping {
            transport_stream_id,
            handle: Some(DurableStreamHandle {
                format_version: 1,
                stream_id: Some(uuid(100 + transport_stream_id)),
                producer_environment_id: Some(EnvironmentId {
                    value: Some(uuid(3)),
                }),
                producer: Some(agent_id()),
                expected_producer_fingerprint: Some(uuid(4)),
                source_invocation: Some(StreamInvocationIdentity {
                    callee_environment_id: Some(EnvironmentId {
                        value: Some(uuid(3)),
                    }),
                    callee: Some(agent_id()),
                    callee_fingerprint: Some(uuid(4)),
                    idempotency_key: key(),
                }),
                component_revision: Some(12),
                element_schema_fingerprint: vec![5; 32],
            }),
            high_water: None,
            role: role as i32,
        }
    }

    fn result(value: SchemaValue) -> InvocationResponse {
        let new_stream_mappings = stream_references(&value)
            .unwrap_or_default()
            .into_iter()
            .map(|stream_id| mapping(stream_id, StreamMappingRole::Output))
            .collect();
        response(invocation_response::Response::Result(
            InvocationSessionResult {
                result: Some(invocation_session_result::Result::MethodResult(value)),
                component_revision: Some(12),
                agent_id: Some(agent_id()),
                idempotency_key: key(),
                new_stream_mappings,
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
                transport_stream_id: 7,
                sequence,
                payload: Some(payload),
                ..Default::default()
            },
        ))
    }

    fn cancel(stream_id: u64, role: StreamCancelRole, offset: u64) -> StreamCancel {
        let server_terminal = matches!(
            role,
            StreamCancelRole::InputConsumer | StreamCancelRole::OutputProducer
        );
        StreamCancel {
            transport_stream_id: stream_id,
            producer_sequence: offset,
            role: role as i32,
            reason: StreamCancelReason::Cancelled as i32,
            details: None,
            durable_stream_id: Some(uuid(100 + stream_id)),
            epoch: 1,
            durable_offset: if server_terminal {
                durable_offset(offset + 10)
            } else {
                Default::default()
            },
        }
    }

    fn durable_offset(sequence: u64) -> Vec<u8> {
        let mut offset = vec![0; 24];
        offset[0] = 1;
        offset[8..16].copy_from_slice(&sequence.to_be_bytes());
        offset
    }

    fn output_item(
        stream_id: u64,
        sequence: u64,
        value: SchemaValue,
        new_stream_mappings: Vec<DurableStreamMapping>,
    ) -> OutputStreamItem {
        OutputStreamItem {
            transport_stream_id: stream_id,
            producer_sequence: sequence,
            value: Some(value),
            durable_stream_id: Some(uuid(100 + stream_id)),
            durable_offset: durable_offset(sequence),
            epoch: 1,
            new_stream_mappings,
        }
    }

    fn output_end(stream_id: u64, sequence: u64) -> OutputStreamEnd {
        OutputStreamEnd {
            transport_stream_id: stream_id,
            producer_sequence: sequence,
            durable_stream_id: Some(uuid(100 + stream_id)),
            durable_offset: durable_offset(sequence),
            epoch: 1,
        }
    }

    fn output_error(stream_id: u64, sequence: u64, details: &str) -> OutputStreamError {
        OutputStreamError {
            transport_stream_id: stream_id,
            producer_sequence: sequence,
            details: details.to_string(),
            durable_stream_id: Some(uuid(100 + stream_id)),
            durable_offset: durable_offset(sequence),
            epoch: 1,
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
        state
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        state
            .validate_public_request(&input_item(0, Payload::PackedU8(vec![1, 2])))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::InputAck(
                InputStreamAck {
                    transport_stream_id: 7,
                    highest_contiguous_sequence: 1,
                    logical_item_count: 2,
                    durable_stream_id: Some(uuid(107)),
                    resulting_offset: durable_offset(1),
                    epoch: 1,
                    ..Default::default()
                },
            )))
            .unwrap();
        state
            .validate_public_request(&public_request(
                public_invocation_request::Request::InputEnd(InputStreamEnd {
                    transport_stream_id: 7,
                    sequence: 2,
                    ..Default::default()
                }),
            ))
            .unwrap();
        state
            .validate_response(&input_ack(7, 2, durable_offset(2)))
            .unwrap();
        state
            .validate_response(&result(record(vec![stream(9), stream(10)])))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::OutputItem(
                output_item(
                    9,
                    0,
                    record(vec![stream(11)]),
                    vec![mapping(11, StreamMappingRole::Output)],
                ),
            )))
            .unwrap();
        for (stream_id, offset) in [(9, 1), (10, 0), (11, 0)] {
            state
                .validate_response(&response(invocation_response::Response::OutputEnd(
                    output_end(stream_id, offset),
                )))
                .unwrap();
        }
        state.validate_response(&success()).unwrap();
        assert!(state.is_complete());
        assert!(state.validate_response(&result(scalar(1))).is_err());
    }

    #[test]
    fn durable_stream_handle_accepts_revision_zero_but_requires_presence() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(vec![stream(7)])))
            .unwrap();
        let mut acceptance = accepted_with_inputs(&[7]);
        let Some(invocation_response::Response::Accepted(accepted)) = acceptance.response.as_mut()
        else {
            unreachable!("accepted helper returned the wrong response")
        };
        accepted.stream_mappings[0]
            .handle
            .as_mut()
            .unwrap()
            .component_revision = Some(0);
        state.validate_response(&acceptance).unwrap();

        let mut missing = mapping(8, StreamMappingRole::Input);
        missing.handle.as_mut().unwrap().component_revision = None;
        assert!(validate_accepted_handle(missing.handle.as_ref().unwrap()).is_err());
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn durable_output_frames_require_identity_epoch_and_offset() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        state.validate_response(&result(stream(9))).unwrap();

        assert!(
            state
                .validate_response(&response(invocation_response::Response::OutputItem(
                    OutputStreamItem {
                        transport_stream_id: 9,
                        producer_sequence: 0,
                        value: Some(scalar(1)),
                        ..Default::default()
                    },
                )))
                .is_err(),
            "a durable output frame without a durable stream ID, epoch, and offset must be rejected"
        );
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn durable_output_stream_identity_cannot_change_between_frames() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        state.validate_response(&result(stream(9))).unwrap();
        state
            .validate_response(&response(invocation_response::Response::OutputItem(
                output_item(9, 0, scalar(1), Vec::new()),
            )))
            .unwrap();

        let mut second = output_item(9, 1, scalar(2), Vec::new());
        second.durable_stream_id = Some(uuid(999));
        assert!(
            state
                .validate_response(&response(
                    invocation_response::Response::OutputItem(second,)
                ))
                .is_err(),
            "one transport stream must not accept frames from two durable stream identities"
        );
    }

    #[test]
    fn durable_output_stream_identity_must_match_the_announced_mapping() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        state.validate_response(&result(stream(9))).unwrap();

        let mut spoofed = output_item(9, 0, scalar(1), Vec::new());
        spoofed.durable_stream_id = Some(uuid(999));
        assert!(
            state
                .validate_response(&response(invocation_response::Response::OutputItem(
                    spoofed,
                )))
                .is_err(),
            "the first frame must not select a durable identity different from the result mapping"
        );
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn durable_output_offsets_must_strictly_increase() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        state.validate_response(&result(stream(9))).unwrap();
        state
            .validate_response(&response(invocation_response::Response::OutputItem(
                output_item(9, 0, scalar(1), Vec::new()),
            )))
            .unwrap();

        let mut second = output_item(9, 1, scalar(2), Vec::new());
        second.durable_offset = durable_offset(0);
        assert!(
            state
                .validate_response(&response(
                    invocation_response::Response::OutputItem(second,)
                ))
                .is_err(),
            "durable output offsets must strictly increase within one stream"
        );
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn durable_input_ack_rejects_a_stale_attachment_epoch() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(vec![stream(7)])))
            .unwrap();
        state
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        state
            .validate_public_request(&input_item(0, Payload::Value(scalar(1))))
            .unwrap();

        let ack = InputStreamAck {
            transport_stream_id: 7,
            highest_contiguous_sequence: 0,
            logical_item_count: 1,
            durable_stream_id: Some(uuid(107)),
            resulting_offset: durable_offset(0),
            epoch: 0,
            new_stream_mappings: Vec::new(),
        };
        assert!(
            state
                .validate_response(&response(invocation_response::Response::InputAck(ack)))
                .is_err(),
            "an acknowledgement from a fenced attachment epoch must be rejected"
        );
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn identical_committed_durable_input_retry_is_accepted() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(vec![stream(7)])))
            .unwrap();
        state
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        let item = input_item(0, Payload::PackedU8(vec![1, 2, 3]));
        state.validate_public_request(&item).unwrap();
        state
            .validate_response(&response(invocation_response::Response::InputAck(
                InputStreamAck {
                    transport_stream_id: 7,
                    highest_contiguous_sequence: 2,
                    logical_item_count: 3,
                    durable_stream_id: Some(uuid(107)),
                    resulting_offset: durable_offset(2),
                    epoch: 1,
                    ..Default::default()
                },
            )))
            .unwrap();

        state.validate_public_request(&item).unwrap();
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn historical_retry_ack_cannot_advance_past_the_accepted_high_water() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(vec![stream(7)])))
            .unwrap();
        let mut acceptance = accepted_with_inputs(&[7]);
        let Some(invocation_response::Response::Accepted(accepted)) = acceptance.response.as_mut()
        else {
            unreachable!("accepted helper returned the wrong response")
        };
        accepted.stream_mappings[0].high_water = Some(InputStreamHighWater {
            highest_contiguous_sequence: 2,
            resulting_offset: durable_offset(2),
            terminal: false,
        });
        state.validate_response(&acceptance).unwrap();
        state
            .validate_public_request(&input_item(0, Payload::Value(scalar(1))))
            .unwrap();

        let replay_ack = InputStreamAck {
            transport_stream_id: 7,
            highest_contiguous_sequence: 0,
            logical_item_count: 1,
            durable_stream_id: Some(uuid(107)),
            resulting_offset: durable_offset(3),
            epoch: 1,
            ..Default::default()
        };
        assert!(
            state
                .validate_response(&response(invocation_response::Response::InputAck(
                    replay_ack,
                )))
                .is_err(),
            "a historical retry ACK cannot claim an offset beyond the accepted durable high-water"
        );
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn conflicting_pending_durable_input_retry_is_rejected() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(vec![stream(7)])))
            .unwrap();
        state
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        state
            .validate_public_request(&input_item(0, Payload::Value(scalar(1))))
            .unwrap();

        assert!(
            state
                .validate_public_request(&input_item(0, Payload::Value(scalar(2))))
                .is_err(),
            "a retry of an unacknowledged range with different content must conflict"
        );
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn overlapping_subrange_of_pending_input_is_rejected() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(vec![stream(7)])))
            .unwrap();
        state
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        state
            .validate_public_request(&input_item(0, Payload::PackedU8(vec![1, 2, 3])))
            .unwrap();

        assert!(
            state
                .validate_public_request(&input_item(1, Payload::PackedU8(vec![9])))
                .is_err(),
            "a subrange with different bytes conflicts with the pending input item it overlaps"
        );
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn each_identical_pending_retry_may_replay_its_ack() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(vec![stream(7)])))
            .unwrap();
        state
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        let item = input_item(0, Payload::PackedU8(vec![1, 2, 3]));
        state.validate_public_request(&item).unwrap();
        state.validate_public_request(&item).unwrap();

        let ack = response(invocation_response::Response::InputAck(InputStreamAck {
            transport_stream_id: 7,
            highest_contiguous_sequence: 2,
            logical_item_count: 3,
            durable_stream_id: Some(uuid(107)),
            resulting_offset: durable_offset(2),
            epoch: 1,
            ..Default::default()
        }));
        state.validate_response(&ack).unwrap();
        assert!(
            state.validate_response(&ack).is_ok(),
            "each admitted identical retry must admit the same replayed ACK"
        );
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn forwarded_input_mapping_preserves_its_original_source_invocation() {
        let mut state = InvocationSessionState::default();
        state
            .validate_trusted_request(&trusted_start(stream(7)))
            .unwrap();

        let mut forwarded = mapping(7, StreamMappingRole::Input);
        let handle = forwarded.handle.as_mut().unwrap();
        handle.producer.as_mut().unwrap().name = "original-producer".to_string();
        let source = handle.source_invocation.as_mut().unwrap();
        source.callee.as_mut().unwrap().name = "original-producer".to_string();
        source.idempotency_key = Some(IdempotencyKey {
            value: "original-invocation".to_string(),
        });

        let mut acceptance = accepted();
        match acceptance.response.as_mut().unwrap() {
            invocation_response::Response::Accepted(accepted) => {
                accepted.stream_mappings = vec![forwarded];
            }
            _ => unreachable!(),
        }

        assert!(
            state.validate_response(&acceptance).is_ok(),
            "a forwarded stream mapping must preserve and accept the original source invocation identity"
        );
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
    fn resume_attach_accepts_the_exact_requested_identity_and_epoch() {
        let resume = resume_attach(vec![StreamCursor {
            stream_id: Some(uuid(107)),
            last_observed_offset: Some(durable_offset(1)),
        }]);
        let mut state = InvocationSessionState::default();
        state.validate_public_request(&resume).unwrap();
        state
            .validate_response(&resumed_acceptance(vec![mapping(
                7,
                StreamMappingRole::Output,
            )]))
            .unwrap();
        assert!(!state.is_complete());
    }

    #[test]
    fn resumed_result_and_nested_item_replay_exact_acceptance_mappings_once() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&resume_attach(Vec::new()))
            .unwrap();
        state
            .validate_response(&resumed_acceptance(vec![
                mapping(7, StreamMappingRole::Output),
                mapping(8, StreamMappingRole::Output),
            ]))
            .unwrap();
        state.validate_response(&result(stream(7))).unwrap();

        let mut nested = output_item(7, 0, stream(8), vec![mapping(8, StreamMappingRole::Output)]);
        nested.epoch = 2;
        state
            .validate_response(&response(invocation_response::Response::OutputItem(nested)))
            .unwrap();

        let mut duplicate =
            output_item(7, 1, stream(8), vec![mapping(8, StreamMappingRole::Output)]);
        duplicate.epoch = 2;
        assert!(
            state
                .validate_response(&response(invocation_response::Response::OutputItem(
                    duplicate,
                )))
                .is_err()
        );
    }

    #[test]
    fn resumed_result_rejects_conflicting_acceptance_mapping_replay() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&resume_attach(Vec::new()))
            .unwrap();
        state
            .validate_response(&resumed_acceptance(vec![mapping(
                7,
                StreamMappingRole::Output,
            )]))
            .unwrap();
        let mut conflicting = result(stream(7));
        let Some(invocation_response::Response::Result(result)) = conflicting.response.as_mut()
        else {
            unreachable!("result helper returned the wrong response")
        };
        result.new_stream_mappings[0]
            .handle
            .as_mut()
            .unwrap()
            .stream_id = Some(uuid(999));
        assert!(state.validate_response(&conflicting).is_err());
    }

    #[test]
    fn resume_acceptance_rejects_identity_and_epoch_mismatches() {
        for mutate in 0..6 {
            let resume = resume_attach(Vec::new());
            let mut acceptance = resumed_acceptance(Vec::new());
            let Some(invocation_response::Response::Accepted(accepted)) =
                acceptance.response.as_mut()
            else {
                unreachable!()
            };
            match mutate {
                0 => accepted.agent_id.as_mut().unwrap().name = "other".to_string(),
                1 => accepted.environment_id.as_mut().unwrap().value = Some(uuid(30)),
                2 => accepted.attachment_id = Some(uuid(31)),
                3 => accepted.attempt_id = Some(uuid(32)),
                4 => accepted.callee_fingerprint = Some(uuid(33)),
                5 => accepted.epoch = 3,
                _ => unreachable!(),
            }
            let mut state = InvocationSessionState::default();
            state.validate_public_request(&resume).unwrap();
            assert!(state.validate_response(&acceptance).is_err());
        }
    }

    #[test]
    fn resume_cursors_must_be_sorted_unique_and_well_formed() {
        let duplicate = resume_attach(vec![
            StreamCursor {
                stream_id: Some(uuid(107)),
                last_observed_offset: None,
            },
            StreamCursor {
                stream_id: Some(uuid(107)),
                last_observed_offset: Some(durable_offset(1)),
            },
        ]);
        assert!(
            InvocationSessionState::default()
                .validate_public_request(&duplicate)
                .is_err()
        );

        let malformed = resume_attach(vec![StreamCursor {
            stream_id: Some(uuid(107)),
            last_observed_offset: Some(vec![1]),
        }]);
        assert!(
            InvocationSessionState::default()
                .validate_public_request(&malformed)
                .is_err()
        );
    }

    #[test]
    fn public_resume_cursor_cannot_control_input_guest_position() {
        let resume = resume_attach(vec![StreamCursor {
            stream_id: Some(uuid(107)),
            last_observed_offset: Some(durable_offset(1)),
        }]);
        let mut state = InvocationSessionState::default();
        state.validate_public_request(&resume).unwrap();

        assert!(
            state
                .validate_response(&resumed_acceptance(vec![mapping(
                    7,
                    StreamMappingRole::Input,
                )]))
                .is_err(),
            "an external input producer cannot supply the guest's input-consumption cursor"
        );
    }

    #[test]
    fn output_resume_cursor_remains_valid_when_handle_is_also_mapped_as_input() {
        let resume = resume_attach(vec![StreamCursor {
            stream_id: Some(uuid(107)),
            last_observed_offset: Some(durable_offset(1)),
        }]);
        let mut state = InvocationSessionState::default();
        state.validate_public_request(&resume).unwrap();

        let input = mapping(7, StreamMappingRole::Input);
        let mut output = mapping(8, StreamMappingRole::Output);
        output.handle = input.handle.clone();

        assert!(
            state
                .validate_response(&resumed_acceptance(vec![input, output]))
                .is_ok(),
            "a role-qualified output mapping must remain resumable when the same forwarded handle is also an input mapping"
        );
    }

    #[test]
    fn attachment_revocation_is_terminal_after_resume_acceptance() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&resume_attach(Vec::new()))
            .unwrap();
        state
            .validate_response(&resumed_acceptance(Vec::new()))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::AttachmentRevoked(
                Default::default(),
            )))
            .unwrap();
        assert!(state.is_complete());
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
        state
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        assert!(
            state
                .validate_public_request(&public_request(
                    public_invocation_request::Request::InputEnd(InputStreamEnd {
                        transport_stream_id: 8,
                        sequence: 0,
                        ..Default::default()
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
                        transport_stream_id: 7,
                        highest_contiguous_sequence: 0,
                        logical_item_count: 2,
                        durable_stream_id: Some(uuid(107)),
                        resulting_offset: durable_offset(0),
                        epoch: 1,
                        ..Default::default()
                    },
                )))
                .is_err()
        );
        state
            .validate_response(&response(invocation_response::Response::InputAck(
                InputStreamAck {
                    transport_stream_id: 7,
                    highest_contiguous_sequence: 2,
                    logical_item_count: 3,
                    durable_stream_id: Some(uuid(107)),
                    resulting_offset: durable_offset(2),
                    epoch: 1,
                    ..Default::default()
                },
            )))
            .unwrap();
        state
            .validate_public_request(&public_request(
                public_invocation_request::Request::InputEnd(InputStreamEnd {
                    transport_stream_id: 7,
                    sequence: 3,
                    ..Default::default()
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
    fn received_output_cancellation_may_race_with_stream_terminal() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(record(Vec::new())))
            .unwrap();
        state.validate_response(&accepted()).unwrap();
        state.validate_response(&result(stream(9))).unwrap();
        state
            .validate_response(&response(invocation_response::Response::OutputEnd(
                output_end(9, 0),
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

        let cancellation = public_request(public_invocation_request::Request::StreamCancel(
            cancel(9, StreamCancelRole::OutputConsumer, 0),
        ));
        state
            .validate_received_public_request(&cancellation)
            .unwrap();
        assert!(
            state
                .validate_received_public_request(&cancellation)
                .is_err()
        );
    }

    #[test]
    fn input_items_register_recursively_nested_streams() {
        let mut state = InvocationSessionState::default();
        state
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        state
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        state
            .validate_public_request(&input_item(0, Payload::Value(record(vec![stream(8)]))))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::InputAck(
                InputStreamAck {
                    transport_stream_id: 7,
                    highest_contiguous_sequence: 0,
                    logical_item_count: 1,
                    durable_stream_id: Some(uuid(107)),
                    resulting_offset: durable_offset(0),
                    epoch: 1,
                    new_stream_mappings: vec![mapping(8, StreamMappingRole::Input)],
                },
            )))
            .unwrap();
        state
            .validate_public_request(&public_request(
                public_invocation_request::Request::InputEnd(InputStreamEnd {
                    transport_stream_id: 7,
                    sequence: 1,
                    ..Default::default()
                }),
            ))
            .unwrap();
        state
            .validate_response(&input_ack(7, 1, durable_offset(1)))
            .unwrap();
        state
            .validate_public_request(&public_request(
                public_invocation_request::Request::InputEnd(InputStreamEnd {
                    transport_stream_id: 8,
                    sequence: 0,
                    ..Default::default()
                }),
            ))
            .unwrap();
        state
            .validate_response(&input_ack(8, 0, durable_offset(0)))
            .unwrap();
        state.validate_response(&result(scalar(1))).unwrap();
        state.validate_response(&success()).unwrap();
    }

    #[test]
    fn input_end_ack_replays_after_loss_and_exact_start_retry() {
        let end = public_request(public_invocation_request::Request::InputEnd(
            InputStreamEnd {
                transport_stream_id: 7,
                sequence: 0,
                ..Default::default()
            },
        ));
        let ack = input_ack(7, 0, durable_offset(0));

        let mut attached = InvocationSessionState::default();
        attached
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        attached
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        attached.validate_public_request(&end).unwrap();
        attached.validate_response(&ack).unwrap();
        attached.validate_public_request(&end).unwrap();
        attached.validate_response(&ack).unwrap();

        let mut retried = InvocationSessionState::default();
        retried
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        let mut acceptance = accepted_with_inputs(&[7]);
        let Some(invocation_response::Response::Accepted(accepted)) = acceptance.response.as_mut()
        else {
            unreachable!("accepted helper returned the wrong response")
        };
        accepted.stream_mappings[0].high_water = Some(InputStreamHighWater {
            highest_contiguous_sequence: 0,
            resulting_offset: durable_offset(2),
            terminal: false,
        });
        retried.validate_response(&acceptance).unwrap();
        retried.validate_public_request(&end).unwrap();
        assert!(
            retried
                .validate_response(&input_ack(7, 0, durable_offset(1)))
                .is_err(),
            "a historical terminal ACK must exactly match the accepted high-water offset"
        );
        retried
            .validate_response(&input_ack(7, 0, durable_offset(2)))
            .unwrap();
    }

    #[test]
    fn all_role_appropriate_cancellations_are_unique_terminals() {
        let mut input = InvocationSessionState::default();
        input
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        input
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        let input_cancel = public_request(public_invocation_request::Request::StreamCancel(
            cancel(7, StreamCancelRole::InputProducer, 0),
        ));
        input.validate_public_request(&input_cancel).unwrap();
        assert!(input.validate_public_request(&input_cancel).is_err());

        let mut input_consumer = InvocationSessionState::default();
        input_consumer
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        input_consumer
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
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
                output_item(9, 0, scalar(1), Vec::new()),
            )))
            .unwrap();
        output
            .validate_response(&response(invocation_response::Response::StreamCancel(
                cancel(9, StreamCancelRole::OutputProducer, 1),
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
        state
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
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
                    transport_stream_id: 7,
                    sequence: 2,
                    ..Default::default()
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
        state
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::StreamCancel(
                cancel(7, StreamCancelRole::InputConsumer, 0),
            )))
            .unwrap();
        let end = public_request(public_invocation_request::Request::InputEnd(
            InputStreamEnd {
                transport_stream_id: 7,
                sequence: 0,
                ..Default::default()
            },
        ));
        state.validate_public_request(&end).unwrap();

        assert!(
            state.validate_public_request(&end).is_err(),
            "an input stream must not accept the producer terminal more than once"
        );
    }

    #[test]
    fn recursive_registration_is_transactional_and_attachment_revocation_is_terminal() {
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
                    output_item(9, 0, stream(9), Vec::new()),
                )))
                .is_err()
        );
        state
            .validate_response(&response(invocation_response::Response::OutputItem(
                output_item(9, 0, scalar(1), Vec::new()),
            )))
            .unwrap();
        state
            .validate_response(&response(invocation_response::Response::AttachmentRevoked(
                Default::default(),
            )))
            .unwrap();
        assert!(state.is_complete());
    }

    #[test]
    fn stream_ids_are_unique_across_input_and_output_directions() {
        let mut input_first = InvocationSessionState::default();
        input_first
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        input_first
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
        assert!(input_first.validate_response(&result(stream(7))).is_err());

        let mut output_first = InvocationSessionState::default();
        output_first
            .validate_public_request(&public_start(stream(7)))
            .unwrap();
        output_first
            .validate_response(&accepted_with_inputs(&[7]))
            .unwrap();
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
    fn public_and_internal_envelopes_round_trip_all_durable_session_variants() {
        round_trip(public_start(record(Vec::new())));
        round_trip(trusted_start(record(Vec::new())));
        round_trip(public_request(
            public_invocation_request::Request::ResumeAttach(ResumeAttach {
                idempotency_key: key(),
                ..Default::default()
            }),
        ));
        round_trip(input_item(4, Payload::PackedU8(vec![1, 2, 3])));
        round_trip(public_request(
            public_invocation_request::Request::InputEnd(InputStreamEnd {
                transport_stream_id: 7,
                sequence: 4,
                ..Default::default()
            }),
        ));
        for role in [
            StreamCancelRole::InputProducer,
            StreamCancelRole::InputConsumer,
            StreamCancelRole::OutputProducer,
            StreamCancelRole::OutputConsumer,
        ] {
            round_trip(StreamCancel {
                transport_stream_id: 7,
                producer_sequence: 8,
                role: role as i32,
                reason: StreamCancelReason::Protocol as i32,
                details: Some("cancelled".to_string()),
                ..Default::default()
            });
        }
        round_trip(accepted());
        round_trip(response(invocation_response::Response::Rejected(
            InvocationRejected {
                reason: InvocationRejectionReason::Validation as i32,
                idempotency_key: key(),
                ..Default::default()
            },
        )));
        round_trip(result(stream(9)));
        round_trip(response(invocation_response::Response::OutputItem(
            output_item(9, 0, scalar(1), Vec::new()),
        )));
        round_trip(response(invocation_response::Response::OutputEnd(
            output_end(9, 1),
        )));
        round_trip(response(invocation_response::Response::OutputError(
            output_error(9, 1, "failed"),
        )));
        round_trip(response(invocation_response::Response::InputAck(
            InputStreamAck {
                transport_stream_id: 7,
                highest_contiguous_sequence: 0,
                logical_item_count: 3,
                ..Default::default()
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
