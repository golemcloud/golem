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

use anyhow::{Context, ensure};
use async_trait::async_trait;
use golem_client::invocation_session::{
    InvocationSession, InvocationSessionRequestProvider, InvocationSessionStateSnapshot,
    ServerFrame, SessionTransportError,
};
use golem_common::model::IdempotencyKey;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::invocation_session_public::{
    INVOCATION_SESSION_VERSION, InvocationSelector, PublicClientMessage, PublicInvocationOutcome,
    PublicInvocationResult, PublicOutputStreamOutcome, PublicResumeOperation, PublicServerMessage,
    PublicStreamDirection, PublicStreamMapping,
};
use golem_common::schema::SchemaValue;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

pub type StreamId = String;

#[derive(Clone, Debug, Default)]
pub struct OutputObservation {
    pub items: Vec<(u64, serde_json::Value)>,
    pub terminal: Option<(u64, PublicOutputStreamOutcome)>,
}

#[derive(Clone, Debug)]
pub struct SessionEvents {
    pub sent: Instant,
    pub accepted: Option<Instant>,
    pub result: Option<Instant>,
    pub first_item: Option<Instant>,
    pub first_items: BTreeMap<StreamId, Instant>,
    pub completed: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionCheckpoint {
    pub idempotency_key: Option<IdempotencyKey>,
    pub result: Option<PublicInvocationResult>,
    pub outputs: BTreeMap<StreamId, OutputObservation>,
    pub attempts: Vec<SessionEvents>,
    channels: BTreeMap<u32, StreamId>,
}

impl SessionCheckpoint {
    pub fn item_count(&self) -> usize {
        self.outputs.values().map(|output| output.items.len()).sum()
    }

    pub fn ensure_success(&self) -> anyhow::Result<()> {
        ensure!(self.result.is_some(), "invocation omitted its result");
        ensure!(
            self.outputs
                .values()
                .all(|output| matches!(output.terminal, Some((_, PublicOutputStreamOutcome::Ok)))),
            "an output stream did not end successfully"
        );
        ensure!(
            self.attempts
                .last()
                .is_some_and(|attempt| attempt.completed.is_some()),
            "invocation did not finish"
        );
        Ok(())
    }

    fn install_mappings(&mut self, mappings: &[PublicStreamMapping]) -> anyhow::Result<()> {
        for mapping in mappings {
            ensure!(
                mapping.direction == PublicStreamDirection::Output,
                "benchmark invocation unexpectedly exposed an input stream"
            );
            if let Some(previous) = self
                .channels
                .insert(mapping.channel, mapping.stream_token.clone())
            {
                ensure!(
                    previous == mapping.stream_token,
                    "stream channel changed identity"
                );
            }
            self.outputs
                .entry(mapping.stream_token.clone())
                .or_default();
        }
        Ok(())
    }

    fn record(&mut self, message: &PublicServerMessage) -> anyhow::Result<()> {
        let now = Instant::now();
        match message {
            PublicServerMessage::InvocationAccepted {
                idempotency_key,
                mappings,
                ..
            } => {
                self.channels.clear();
                self.install_mappings(mappings)?;
                let key = IdempotencyKey::new(idempotency_key.clone());
                if let Some(previous) = &self.idempotency_key {
                    ensure!(previous == &key, "reattachment changed invocation identity");
                }
                self.idempotency_key = Some(key);
                self.attempts
                    .last_mut()
                    .context("missing request timing")?
                    .accepted = Some(now);
            }
            PublicServerMessage::InvocationResult {
                mappings, result, ..
            } => {
                self.install_mappings(mappings)?;
                self.result = Some((**result).clone());
                self.attempts
                    .last_mut()
                    .context("missing request timing")?
                    .result = Some(now);
            }
            PublicServerMessage::OutputStreamItem {
                channel,
                mappings,
                sequence,
                value,
                ..
            } => {
                self.install_mappings(mappings)?;
                let stream = self
                    .channels
                    .get(channel)
                    .context("output item has no mapping")?
                    .clone();
                let output = self
                    .outputs
                    .get_mut(&stream)
                    .context("output stream is absent")?;
                ensure!(
                    output
                        .items
                        .last()
                        .is_none_or(|(previous, _)| previous + 1 == sequence.0),
                    "output sequence is not contiguous"
                );
                output.items.push((sequence.0, value.clone()));
                let attempt = self.attempts.last_mut().context("missing request timing")?;
                attempt.first_item.get_or_insert(now);
                attempt.first_items.entry(stream).or_insert(now);
            }
            PublicServerMessage::OutputStreamEnd {
                channel,
                sequence,
                outcome,
                ..
            } => {
                let stream = self
                    .channels
                    .get(channel)
                    .context("output terminal has no mapping")?;
                let output = self
                    .outputs
                    .get_mut(stream)
                    .context("output stream is absent")?;
                ensure!(output.terminal.is_none(), "duplicate output terminal");
                output.terminal = Some((sequence.0, outcome.clone()));
            }
            PublicServerMessage::InvocationFinished { outcome, .. } => {
                ensure!(
                    *outcome == PublicInvocationOutcome::Success,
                    "invocation failed: {outcome:?}"
                );
                self.attempts
                    .last_mut()
                    .context("missing request timing")?
                    .completed = Some(now);
            }
            PublicServerMessage::InvocationRejected { code, message, .. } => {
                anyhow::bail!("invocation rejected ({code:?}): {message}")
            }
            PublicServerMessage::AttachmentRevoked { reason, .. } => {
                anyhow::bail!("attachment revoked: {reason:?}")
            }
            PublicServerMessage::InputStreamAck { .. }
            | PublicServerMessage::StreamCancel { .. } => {
                anyhow::bail!("unexpected input or cancellation response")
            }
        }
        Ok(())
    }
}

struct RequestProvider {
    url: String,
    token: TokenSecret,
}

#[async_trait]
impl InvocationSessionRequestProvider for RequestProvider {
    async fn request(
        &self,
    ) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, SessionTransportError> {
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(SessionTransportError::Request)?;
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {}", self.token.secret())
                .parse()
                .map_err(|error| {
                    SessionTransportError::RequestProvider(format!(
                        "invalid authorization token: {error}"
                    ))
                })?,
        );
        Ok(request)
    }
}

pub struct DetachedSession {
    pub checkpoint: SessionCheckpoint,
    state: InvocationSessionStateSnapshot,
}

pub struct PublicInvocationSession {
    inner: InvocationSession,
    checkpoint: SessionCheckpoint,
    deadline: tokio::time::Instant,
}

impl PublicInvocationSession {
    pub async fn start(
        deps: &BenchmarkTestDependencies,
        token: &TokenSecret,
        application: &str,
        environment: &str,
        agent: &ParsedAgentId,
        method: &str,
        method_parameters: serde_json::Value,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        let SchemaValue::Record { fields } = agent.parameters.value() else {
            anyhow::bail!("benchmark agent constructor is not a record")
        };
        let [SchemaValue::String(name)] = fields.as_slice() else {
            anyhow::bail!("benchmark agent constructor is not a single name")
        };
        let constructor_parameters = serde_json::json!({ "name": name });
        let key = IdempotencyKey::fresh();
        let start = PublicClientMessage::InvocationStart {
            attempt_id: uuid::Uuid::new_v4(),
            config: Vec::new(),
            idempotency_key: key.value.clone(),
            method_parameters,
            selector: Box::new(InvocationSelector {
                agent_type: agent.agent_type.to_string(),
                application: application.to_string(),
                constructor_parameters,
                environment: environment.to_string(),
                method: method.to_string(),
                phantom_id: agent.phantom_id,
            }),
            version: INVOCATION_SESSION_VERSION,
        };
        Self::open(
            deps,
            token,
            InvocationSessionStateSnapshot {
                delivered_output_cursors: BTreeMap::new(),
                pending_operation: Some(start),
                session_token: None,
            },
            SessionCheckpoint {
                idempotency_key: Some(key),
                ..Default::default()
            },
            timeout,
        )
        .await
    }

    pub async fn resume(
        deps: &BenchmarkTestDependencies,
        token: &TokenSecret,
        detached: &DetachedSession,
        operation: PublicResumeOperation,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        let mut state = detached.state.clone();
        state.pending_operation = Some(PublicClientMessage::ResumeAttach {
            attempt_id: uuid::Uuid::new_v4(),
            operation,
            output_cursors: state.delivered_output_cursors.values().cloned().collect(),
            session_token: state
                .session_token
                .clone()
                .context("session token is absent")?,
            version: INVOCATION_SESSION_VERSION,
        });
        Self::open(deps, token, state, detached.checkpoint.clone(), timeout).await
    }

    async fn open(
        deps: &BenchmarkTestDependencies,
        token: &TokenSecret,
        state: InvocationSessionStateSnapshot,
        mut checkpoint: SessionCheckpoint,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        checkpoint.attempts.push(SessionEvents {
            sent: Instant::now(),
            accepted: None,
            result: None,
            first_item: None,
            first_items: BTreeMap::new(),
            completed: None,
        });
        let worker_service = deps.worker_service();
        let provider = Arc::new(RequestProvider {
            url: format!(
                "ws://{}:{}/v1/agents/invoke-agent-session",
                worker_service.http_host(),
                worker_service.http_port()
            ),
            token: token.clone(),
        });
        let deadline = tokio::time::Instant::now() + timeout;
        let inner = tokio::time::timeout_at(
            deadline,
            InvocationSession::open(provider, None, state, false, Arc::new(())),
        )
        .await
        .context("opening public invocation session timed out")??;
        let mut session = Self {
            inner,
            checkpoint,
            deadline,
        };
        while !session.inner.is_accepted() {
            session.receive().await?;
        }
        Ok(session)
    }

    pub fn checkpoint(&self) -> &SessionCheckpoint {
        &self.checkpoint
    }

    pub async fn receive(&mut self) -> anyhow::Result<()> {
        let mut frame = tokio::time::timeout_at(self.deadline, self.inner.receive())
            .await
            .context("receiving public invocation response timed out")??;
        let ServerFrame::Message(message) = frame.frame() else {
            anyhow::bail!("benchmark invocation returned an unexpected binary frame")
        };
        self.checkpoint.record(message)?;
        frame.mark_delivered()?;
        Ok(())
    }

    pub async fn disconnect(self) -> anyhow::Result<DetachedSession> {
        let state = self.inner.state();
        Ok(DetachedSession {
            checkpoint: self.checkpoint,
            state,
        })
    }

    pub async fn disconnect_after(mut self, items: usize) -> anyhow::Result<DetachedSession> {
        while self.checkpoint.item_count() < items {
            self.receive().await?;
        }
        self.disconnect().await
    }

    pub async fn finish(mut self) -> anyhow::Result<SessionCheckpoint> {
        while self
            .checkpoint
            .attempts
            .last()
            .is_some_and(|attempt| attempt.completed.is_none())
        {
            self.receive().await?;
        }
        self.checkpoint.ensure_success()?;
        Ok(self.checkpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(channel: u32, stream_token: &str) -> PublicStreamMapping {
        PublicStreamMapping {
            channel,
            direction: PublicStreamDirection::Output,
            byte_role: None,
            input_high_water: None,
            provisional_ref: None,
            stream_token: stream_token.to_string(),
        }
    }

    fn accepted(channel: u32, stream_token: &str) -> PublicServerMessage {
        PublicServerMessage::InvocationAccepted {
            attempt_id: uuid::Uuid::new_v4(),
            idempotency_key: "invocation".to_string(),
            mappings: vec![mapping(channel, stream_token)],
            session_token: "session".to_string(),
            version: INVOCATION_SESSION_VERSION,
        }
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test_r::test]
    fn reconnect_accepts_connection_local_channel_renumbering() {
        let event = || SessionEvents {
            sent: Instant::now(),
            accepted: None,
            result: None,
            first_item: None,
            first_items: BTreeMap::new(),
            completed: None,
        };
        let mut checkpoint = SessionCheckpoint {
            idempotency_key: Some(IdempotencyKey::new("invocation".to_string())),
            attempts: vec![event()],
            ..Default::default()
        };
        checkpoint.record(&accepted(1, "stream")).unwrap();

        checkpoint.attempts.push(event());
        checkpoint.record(&accepted(2, "stream")).unwrap();
    }
}
