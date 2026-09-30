// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use golem_api_grpc::proto::golem::schema::{ListValue, SchemaValue, schema_value};
use golem_api_grpc::proto::golem::worker::input_stream_item;
use golem_api_grpc::proto::golem::worker::{
    DurableStreamMapping, InputStreamEnd, InputStreamItem, InvocationAccepted, InvocationRequest,
    ResumeAttach, StreamCancelReason, StreamMappingRole, invocation_request,
    invocation_session_result,
};
use prost::Message;
use std::collections::{HashMap, HashSet, VecDeque};
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::Instant;

use super::driver::{output_cancel_request, request_end_sequence, uuid_pair};
use super::{HttpSessionError, HttpSessionLimits, INPUT_STREAM_ID};
pub(super) struct InputCredits {
    pub(super) _bytes: OwnedSemaphorePermit,
    pub(super) _frame: OwnedSemaphorePermit,
}

pub(super) enum Command {
    Input(Vec<u8>, InputCredits),
    InputEof(InputCredits),
    DisposeInput,
    DisposeOutput { stream_id: u64 },
}

pub(super) struct PreparedCommand {
    pub(super) request: InvocationRequest,
    pub(super) credits: Option<InputCredits>,
}

impl Command {
    pub(super) fn prepare(
        self,
        input: &mut InputProgress,
        output: &mut OutputProgress,
        epoch: u64,
    ) -> Result<Option<PreparedCommand>, HttpSessionError> {
        let (request, credits) = match self {
            Self::Input(bytes, credits) => {
                if input.terminal || input.disposal_pending {
                    return Ok(None);
                }
                let sequence = input.next_sequence;
                input.next_sequence += 1;
                let item = InputStreamItem {
                    transport_stream_id: INPUT_STREAM_ID,
                    sequence,
                    payload: Some(input_stream_item::Payload::Value(SchemaValue {
                        value: Some(schema_value::Value::ListValue(ListValue {
                            elements: bytes
                                .into_iter()
                                .map(|byte| SchemaValue {
                                    value: Some(schema_value::Value::U8Value(byte as u32)),
                                })
                                .collect(),
                        })),
                    })),
                    durable_stream_id: None,
                    epoch: 0,
                };
                (
                    InvocationRequest {
                        request: Some(invocation_request::Request::InputItem(item)),
                    },
                    Some(credits),
                )
            }
            Self::InputEof(credits) => {
                if input.terminal || input.disposal_pending {
                    return Ok(None);
                }
                input.terminal = true;
                (
                    InvocationRequest {
                        request: Some(invocation_request::Request::InputEnd(InputStreamEnd {
                            transport_stream_id: INPUT_STREAM_ID,
                            sequence: input.next_sequence,
                            durable_stream_id: None,
                            epoch: 0,
                        })),
                    },
                    Some(credits),
                )
            }
            Self::DisposeInput => {
                if !input.terminal {
                    input.disposal_pending = true;
                }
                return Ok(None);
            }
            Self::DisposeOutput { stream_id } => {
                let durable_id = *output.controls.get(&stream_id).ok_or_else(|| {
                    HttpSessionError::Protocol(format!("output stream {stream_id} is unknown"))
                })?;
                if output.terminals.contains(&durable_id)
                    || !output.pending_disposals.insert(stream_id)
                {
                    return Ok(None);
                }
                (
                    output_cancel_request(
                        stream_id,
                        durable_id,
                        epoch,
                        StreamCancelReason::ConsumerDrop,
                    ),
                    None,
                )
            }
        };
        Ok(Some(PreparedCommand { request, credits }))
    }
}
pub(super) struct RetainedInput {
    pub(super) request: InvocationRequest,
    encoded_bytes: usize,
    end_sequence: u64,
    _permit: Option<InputCredits>,
}

pub(super) struct AcceptedIdentity {
    pub(super) accepted: InvocationAccepted,
    pub(super) input_mapping: DurableStreamMapping,
}

#[derive(Default)]
pub(super) struct InputProgress {
    pub(super) retained: VecDeque<RetainedInput>,
    pub(super) retained_bytes: usize,
    pub(super) next_sequence: u64,
    pub(super) terminal: bool,
    pub(super) disposal_pending: bool,
}

impl InputProgress {
    pub(super) fn can_read(&self, limits: &HttpSessionLimits) -> bool {
        self.retained.len() < limits.retained_input_frames
            && self.retained_bytes < limits.retained_input_bytes
    }

    pub(super) fn acknowledge(&mut self, sequence: u64) {
        while self
            .retained
            .front()
            .is_some_and(|frame| frame.end_sequence <= sequence)
        {
            self.retained_bytes -= self.retained.pop_front().unwrap().encoded_bytes;
        }
    }

    pub(super) fn clear(&mut self) {
        self.retained.clear();
        self.retained_bytes = 0;
    }

    pub(super) fn apply_high_water(&mut self, mapping: &DurableStreamMapping) {
        if self.disposal_pending {
            self.terminal = mapping
                .high_water
                .as_ref()
                .is_some_and(|water| water.terminal);
        }
        if let Some(high_water) = mapping.high_water.as_ref() {
            if high_water.terminal {
                self.terminal = true;
                self.clear();
            }
            self.acknowledge(high_water.highest_contiguous_sequence);
        }
    }

    pub(super) fn retain(
        &mut self,
        request: InvocationRequest,
        permit: Option<InputCredits>,
        limits: &HttpSessionLimits,
    ) -> Result<(), HttpSessionError> {
        if permit.is_some() && self.retained.len() >= limits.retained_input_frames {
            return Err(HttpSessionError::RetainedInputLimit);
        }
        let retain = permit.is_some()
            || matches!(
                request.request,
                Some(invocation_request::Request::InputEnd(_))
            );
        if retain {
            let encoded_bytes = request.encoded_len();
            self.retained.push_back(RetainedInput {
                end_sequence: request_end_sequence(&request),
                request,
                encoded_bytes,
                _permit: permit,
            });
            self.retained_bytes += encoded_bytes;
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct OutputProgress {
    pub(super) cursors: HashMap<(u64, u64), Vec<u8>>,
    pub(super) controls: HashMap<u64, (u64, u64)>,
    pub(super) terminals: HashSet<(u64, u64)>,
    pub(super) pending_disposals: HashSet<u64>,
    pub(super) result_value: Option<Option<invocation_session_result::Result>>,
}

impl OutputProgress {
    pub(super) fn observe(&mut self, stream_id: u64, durable_id: (u64, u64), offset: Vec<u8>) {
        self.cursors.insert(durable_id, offset);
        self.controls.insert(stream_id, durable_id);
    }

    pub(super) fn terminal(&mut self, stream_id: u64, durable_id: (u64, u64), offset: Vec<u8>) {
        self.observe(stream_id, durable_id, offset);
        self.terminals.insert(durable_id);
        self.pending_disposals.remove(&stream_id);
    }

    pub(super) fn discover_result_streams(
        &mut self,
        mappings: &[DurableStreamMapping],
    ) -> Vec<(u64, (u64, u64))> {
        let mut discovered = Vec::new();
        for mapping in mappings {
            if mapping.role() == StreamMappingRole::Output
                && let Some(id) = mapping
                    .handle
                    .as_ref()
                    .and_then(|handle| handle.stream_id.as_ref())
            {
                let id = uuid_pair(id);
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.controls.entry(mapping.transport_stream_id)
                {
                    entry.insert(id);
                    discovered.push((mapping.transport_stream_id, id));
                }
            }
        }
        discovered
    }
}

#[derive(Default)]
pub(super) struct RecoveryProgress {
    pub(super) deadline: Option<Instant>,
    pub(super) started: Option<Instant>,
    pub(super) attempts: usize,
    /// The executor may commit this attempt before its acceptance reaches us. Transport loss is
    /// therefore ambiguous: replay the exact request until an acceptance or definite rejection.
    pub(super) pending: Option<ResumeAttach>,
}
