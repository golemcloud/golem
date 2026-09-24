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

//! Trusted output invocations through worker-service, including exact-cursor reattachment.
//! Observations survive connections and are keyed by durable identity, never transport channel.

use anyhow::{Context, bail, ensure};
use futures::{StreamExt, stream::BoxStream};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::common::Uuid;
use golem_api_grpc::proto::golem::schema::SchemaValue as ProtoValue;
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::{
    AgentInvocationMode, DurableStreamHandle, DurableStreamMapping, InvocationAccepted,
    InvocationRequest, InvocationResponse, InvocationSessionCompletion, InvocationSessionResult,
    InvocationStart, OutputStreamItem, ResumeAttach, ResumeOperation, StreamCursor,
    StreamMappingRole, invocation_request, invocation_response, invocation_session_completion,
};
use golem_client::model::ComponentDto;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::schema::TypedSchemaValue;
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::config::TestDependencies;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

pub type StreamId = (u64, u64);

fn stream_id(id: Uuid) -> StreamId {
    (id.high_bits, id.low_bits)
}

/// One request's timing boundaries. A resumed first item is timed from Resume/Takeover send,
/// including acceptance and the caller's subsequent gate release.
#[derive(Debug, Clone)]
pub struct SessionEvents {
    pub sent: Instant,
    pub accepted: Option<Instant>,
    pub result: Option<Instant>,
    pub first_item: Option<Instant>,
    pub first_items: BTreeMap<StreamId, Instant>,
    pub completed: Option<Instant>,
}

#[derive(Debug, Clone, Default)]
pub struct OutputObservation {
    pub handle: Option<DurableStreamHandle>,
    pub items: Vec<OutputStreamItem>,
    /// The complete terminal protobuf retains its offset, sequence, epoch and outcome.
    pub terminal: Option<InvocationResponse>,
    pub cursor: Option<Vec<u8>>,
    offsets: BTreeSet<Vec<u8>>,
}

impl OutputObservation {
    fn advance(&mut self, offset: &[u8]) -> anyhow::Result<()> {
        ensure!(self.terminal.is_none(), "output after terminal");
        ensure!(!offset.is_empty(), "output omitted its durable offset");
        ensure!(
            self.offsets.insert(offset.to_vec()),
            "duplicate durable offset"
        );
        self.cursor = Some(offset.to_vec());
        Ok(())
    }

    pub fn values(&self) -> anyhow::Result<Vec<ProtoValue>> {
        self.items
            .iter()
            .map(|item| {
                item.value
                    .clone()
                    .context("expected a typed value, received packed bytes")
            })
            .collect()
    }
}

/// A detached checkpoint is owned by the caller; dropping a connection never sends cancellation.
/// Keep this entire value, not just its cursors, to detect duplicate offsets across reconnects.
#[derive(Debug, Clone, Default)]
pub struct SessionCheckpoint {
    pub acceptance: Option<InvocationAccepted>,
    pub outputs: BTreeMap<StreamId, OutputObservation>,
    pub mappings: BTreeMap<u64, DurableStreamMapping>,
    pub result: Option<InvocationSessionResult>,
    pub completion: Option<InvocationSessionCompletion>,
    pub attempts: Vec<SessionEvents>,
}

impl SessionCheckpoint {
    pub fn cursors(&self) -> Vec<StreamCursor> {
        self.outputs
            .iter()
            .map(|(&(high_bits, low_bits), output)| StreamCursor {
                stream_id: Some(Uuid {
                    high_bits,
                    low_bits,
                }),
                last_observed_offset: output.cursor.clone(),
            })
            .collect()
    }

    pub fn item_count(&self) -> usize {
        self.outputs.values().map(|output| output.items.len()).sum()
    }

    pub fn ensure_success(&self) -> anyhow::Result<()> {
        ensure!(
            matches!(
                self.completion.as_ref().and_then(|c| c.outcome.as_ref()),
                Some(invocation_session_completion::Outcome::Success(_))
            ),
            "invocation did not succeed: {:?}",
            self.completion
        );
        ensure!(
            self.result.is_some(),
            "successful invocation omitted result"
        );
        ensure!(
            self.outputs.values().all(|output| matches!(
                output.terminal.as_ref().and_then(|t| t.response.as_ref()),
                Some(invocation_response::Response::OutputEnd(_))
            )),
            "an output did not end successfully"
        );
        Ok(())
    }

    fn remember_mappings(&mut self, mappings: &[DurableStreamMapping]) -> anyhow::Result<()> {
        for mapping in mappings {
            ensure!(
                mapping.role == StreamMappingRole::Output as i32,
                "output-session driver does not support input streams"
            );
            let id = stream_id(
                mapping
                    .handle
                    .as_ref()
                    .and_then(|h| h.stream_id)
                    .context("mapping omitted durable stream identity")?,
            );
            let output = self.outputs.entry(id).or_default();
            if let Some(handle) = &output.handle {
                ensure!(
                    Some(handle) == mapping.handle.as_ref(),
                    "reattachment changed durable stream handle"
                );
            }
            output.handle = mapping.handle.clone();
            self.mappings
                .insert(mapping.transport_stream_id, mapping.clone());
        }
        Ok(())
    }

    fn record(&mut self, response: &InvocationResponse) -> anyhow::Result<()> {
        let now = Instant::now();
        match response
            .response
            .as_ref()
            .context("empty invocation response")?
        {
            invocation_response::Response::Accepted(accepted) => {
                if let Some(previous) = &self.acceptance {
                    ensure!(
                        accepted.epoch == previous.epoch + 1,
                        "reattachment must increment epoch"
                    );
                    ensure!(
                        accepted.attachment_id == previous.attachment_id
                            && accepted.callee_fingerprint == previous.callee_fingerprint
                            && accepted.agent_id == previous.agent_id
                            && accepted.idempotency_key == previous.idempotency_key,
                        "reattachment changed session identity"
                    );
                }
                self.remember_mappings(&accepted.stream_mappings)?;
                self.acceptance = Some(accepted.clone());
                self.attempts
                    .last_mut()
                    .context("no request timing")?
                    .accepted = Some(now);
            }
            invocation_response::Response::Result(result) => {
                self.remember_mappings(&result.new_stream_mappings)?;
                self.result = Some(result.clone());
                self.attempts
                    .last_mut()
                    .context("no request timing")?
                    .result = Some(now);
            }
            invocation_response::Response::OutputItem(item) => {
                self.remember_mappings(&item.new_stream_mappings)?;
                let output = self
                    .outputs
                    .get_mut(&stream_id(
                        item.durable_stream_id
                            .context("output item omitted durable identity")?,
                    ))
                    .context("output item has no mapping")?;
                output.advance(&item.durable_offset)?;
                output.items.push(item.clone());
                let events = self.attempts.last_mut().context("no request timing")?;
                events.first_item.get_or_insert(now);
                events
                    .first_items
                    .entry(stream_id(item.durable_stream_id.unwrap()))
                    .or_insert(now);
            }
            invocation_response::Response::OutputEnd(end) => {
                self.record_terminal(end.durable_stream_id, &end.durable_offset, response)?;
            }
            invocation_response::Response::OutputError(error) => {
                self.record_terminal(error.durable_stream_id, &error.durable_offset, response)?;
            }
            invocation_response::Response::StreamCancel(cancel) => {
                self.record_terminal(cancel.durable_stream_id, &cancel.durable_offset, response)?;
            }
            invocation_response::Response::Finished(finished) => {
                self.completion = Some(finished.clone());
                self.attempts
                    .last_mut()
                    .context("no request timing")?
                    .completed = Some(now);
            }
            invocation_response::Response::Rejected(rejected) => {
                bail!("session rejected: {rejected:?}")
            }
            invocation_response::Response::AttachmentRevoked(revoked) => {
                bail!("attachment revoked: {revoked:?}")
            }
            invocation_response::Response::InputAck(_) => bail!("unexpected input acknowledgement"),
        }
        Ok(())
    }

    fn record_terminal(
        &mut self,
        id: Option<Uuid>,
        offset: &[u8],
        response: &InvocationResponse,
    ) -> anyhow::Result<()> {
        let output = self
            .outputs
            .get_mut(&stream_id(id.context("terminal omitted durable identity")?))
            .context("terminal has no mapping")?;
        output.advance(offset)?;
        output.terminal = Some(response.clone());
        Ok(())
    }
}

pub struct InvocationSession {
    // Retain the request half until disconnect or completion, without sending cancellation.
    _requests: mpsc::Sender<InvocationRequest>,
    responses: BoxStream<'static, anyhow::Result<InvocationResponse>>,
    state: InvocationSessionState,
    deadline: tokio::time::Instant,
    checkpoint: SessionCheckpoint,
}

impl InvocationSession {
    /// Connect before the request timer starts; all remaining work shares one absolute deadline.
    pub async fn start(
        deps: &impl TestDependencies,
        component: &ComponentDto,
        agent: &ParsedAgentId,
        method: &str,
        input: TypedSchemaValue,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        let agent = AgentId::from_agent_id(component.id, agent).map_err(anyhow::Error::msg)?;
        let (_, value) = input.into_parts();
        let request = InvocationRequest {
            request: Some(invocation_request::Request::Start(InvocationStart {
                agent_id: Some(agent.into()),
                method_name: Some(method.to_string()),
                input: Some(value.try_into().map_err(anyhow::Error::msg)?),
                idempotency_key: Some(IdempotencyKey::fresh().into()),
                auth_ctx: Some(AuthCtx::System.into()),
                environment_id: Some(component.environment_id.into()),
                mode: AgentInvocationMode::Await as i32,
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                ..Default::default()
            })),
        };
        Self::connect(deps, request, SessionCheckpoint::default(), timeout).await
    }

    /// Returns after validated acceptance. Release producer gates only after this returns.
    pub async fn resume(
        deps: &impl TestDependencies,
        checkpoint: SessionCheckpoint,
        operation: ResumeOperation,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        ensure!(
            matches!(
                operation,
                ResumeOperation::Resume | ResumeOperation::Takeover
            ),
            "invalid resume operation"
        );
        let accepted = checkpoint
            .acceptance
            .as_ref()
            .context("cannot resume before acceptance")?;
        let request = InvocationRequest {
            request: Some(invocation_request::Request::ResumeAttach(ResumeAttach {
                idempotency_key: accepted.idempotency_key.clone(),
                agent_id: accepted.agent_id.clone(),
                environment_id: accepted.environment_id,
                attachment_id: accepted.attachment_id,
                attempt_id: Some(uuid::Uuid::new_v4().into()),
                expected_callee_fingerprint: accepted.callee_fingerprint,
                expected_epoch: accepted.epoch,
                operation: operation as i32,
                cursors: checkpoint.cursors(),
                auth_ctx: Some(AuthCtx::System.into()),
                principal: None,
            })),
        };
        Self::connect(deps, request, checkpoint, timeout).await
    }

    async fn connect(
        deps: &impl TestDependencies,
        request: InvocationRequest,
        mut checkpoint: SessionCheckpoint,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut state = InvocationSessionState::default();
        state
            .validate_trusted_request(&request)
            .map_err(anyhow::Error::msg)?;
        for (id, output) in &checkpoint.outputs {
            if output.terminal.is_some() {
                state
                    .mark_terminal_resume_cursor(*id)
                    .map_err(anyhow::Error::msg)?;
            }
        }
        let worker_service = deps.worker_service();
        let mut client = tokio::time::timeout_at(
            deadline,
            WorkerServiceClient::connect(format!(
                "http://{}:{}",
                worker_service.grpc_host(),
                worker_service.gprc_port()
            )),
        )
        .await
        .context("worker-service connection deadline")??;
        let (requests, receiver) = mpsc::channel(1);
        checkpoint.mappings.clear();
        checkpoint.completion = None;
        checkpoint.attempts.push(SessionEvents {
            sent: Instant::now(),
            accepted: None,
            result: None,
            first_item: None,
            first_items: BTreeMap::new(),
            completed: None,
        });
        requests.send(request).await?;
        let inbound = tokio::time::timeout_at(
            deadline,
            client.invoke_agent_session(ReceiverStream::new(receiver)),
        )
        .await
        .context("invocation open deadline")??
        .into_inner();
        let mut session = Self {
            _requests: requests,
            responses: inbound
                .map(|response| response.map_err(anyhow::Error::from))
                .boxed(),
            state,
            deadline,
            checkpoint,
        };
        let response = session.receive().await?;
        ensure!(
            matches!(
                response.response,
                Some(invocation_response::Response::Accepted(_))
            ),
            "expected acceptance"
        );
        Ok(session)
    }

    pub fn checkpoint(&self) -> &SessionCheckpoint {
        &self.checkpoint
    }

    pub async fn receive(&mut self) -> anyhow::Result<InvocationResponse> {
        let response = tokio::time::timeout_at(self.deadline, self.responses.next())
            .await
            .context("invocation phase deadline")?
            .context("connection ended before invocation completion")??;
        self.state
            .validate_response(&response)
            .map_err(anyhow::Error::msg)?;
        self.checkpoint.record(&response)?;
        Ok(response)
    }

    /// Observe exactly this many additional item frames, then drop transport without cancellation.
    pub async fn disconnect_after(mut self, items: usize) -> anyhow::Result<SessionCheckpoint> {
        let target = self.checkpoint.item_count() + items;
        while self.checkpoint.item_count() < target {
            ensure!(
                !self.state.is_complete(),
                "invocation completed before checkpoint"
            );
            self.receive().await?;
        }
        Ok(self.disconnect())
    }

    pub fn disconnect(self) -> SessionCheckpoint {
        self.checkpoint
    }

    pub async fn finish(mut self) -> anyhow::Result<SessionCheckpoint> {
        while !self.state.is_complete() {
            self.receive().await?;
        }
        ensure!(
            tokio::time::timeout_at(self.deadline, self.responses.next())
                .await
                .context("completed invocation did not close")?
                .is_none(),
            "frame after completion"
        );
        self.checkpoint.ensure_success()?;
        Ok(self.checkpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::golem::worker::OutputStreamEnd;
    use test_r::test;

    fn checkpoint() -> SessionCheckpoint {
        let mut checkpoint = SessionCheckpoint::default();
        checkpoint.attempts.push(SessionEvents {
            sent: Instant::now(),
            accepted: None,
            result: None,
            first_item: None,
            first_items: BTreeMap::new(),
            completed: None,
        });
        checkpoint
            .remember_mappings(
                &[(1, 9), (2, 5)]
                    .into_iter()
                    .enumerate()
                    .map(|(channel, (high_bits, low_bits))| DurableStreamMapping {
                        transport_stream_id: channel as u64 + 1,
                        handle: Some(DurableStreamHandle {
                            stream_id: Some(Uuid {
                                high_bits,
                                low_bits,
                            }),
                            ..Default::default()
                        }),
                        role: StreamMappingRole::Output as i32,
                        ..Default::default()
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        checkpoint
    }

    fn item(id: StreamId, offset: u8) -> InvocationResponse {
        InvocationResponse {
            response: Some(invocation_response::Response::OutputItem(
                OutputStreamItem {
                    durable_stream_id: Some(Uuid {
                        high_bits: id.0,
                        low_bits: id.1,
                    }),
                    durable_offset: vec![offset; 24],
                    value: Some(
                        ProtoValue::try_from(golem_common::schema::SchemaValue::U32(u32::from(
                            offset,
                        )))
                        .unwrap(),
                    ),
                    ..Default::default()
                },
            )),
        }
    }

    #[test]
    fn cursor_accumulation_keeps_each_stream_and_terminal() {
        let mut checkpoint = checkpoint();
        checkpoint.record(&item((1, 9), 3)).unwrap();
        checkpoint.record(&item((2, 5), 4)).unwrap();
        checkpoint.record(&item((1, 9), 7)).unwrap();
        let terminal = InvocationResponse {
            response: Some(invocation_response::Response::OutputEnd(OutputStreamEnd {
                durable_stream_id: Some(Uuid {
                    high_bits: 1,
                    low_bits: 9,
                }),
                durable_offset: vec![11; 24],
                ..Default::default()
            })),
        };
        checkpoint.record(&terminal).unwrap();
        assert_eq!(
            checkpoint.cursors(),
            vec![
                StreamCursor {
                    stream_id: Some(Uuid {
                        high_bits: 1,
                        low_bits: 9
                    }),
                    last_observed_offset: Some(vec![11; 24])
                },
                StreamCursor {
                    stream_id: Some(Uuid {
                        high_bits: 2,
                        low_bits: 5
                    }),
                    last_observed_offset: Some(vec![4; 24])
                },
            ]
        );
        assert_eq!(checkpoint.outputs[&(1, 9)].terminal, Some(terminal));
        assert_eq!(checkpoint.outputs[&(1, 9)].items.len(), 2);
        assert_eq!(checkpoint.outputs[&(2, 5)].items.len(), 1);
        assert!(checkpoint.record(&item((1, 9), 12)).is_err());
    }

    #[test]
    fn cursor_rejects_duplicate_offset_across_checkpoint_clone() {
        let mut checkpoint = checkpoint();
        checkpoint.record(&item((1, 9), 3)).unwrap();
        checkpoint.record(&item((1, 9), 4)).unwrap();
        let mut resumed = checkpoint.clone();
        assert!(resumed.record(&item((1, 9), 3)).is_err());
        assert_eq!(resumed.outputs[&(1, 9)].cursor, Some(vec![4; 24]));
        assert_eq!(resumed.outputs[&(1, 9)].items.len(), 2);
        // The same offset bytes in a different stream are not a duplicate.
        resumed.record(&item((2, 5), 3)).unwrap();
        resumed.record(&item((1, 9), 5)).unwrap();
    }
}
