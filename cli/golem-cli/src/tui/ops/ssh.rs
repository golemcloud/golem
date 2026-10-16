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

use crate::command_handler::{EmbeddedSshResult, SshSession};
use crate::tui::context_executor::TuiContextId;
use crate::tui::ops::AgentIdentity;
use std::cell::Cell;
use std::time::Duration;
use tokio::sync::oneshot;

const MAX_TRANSCRIPT_LINES: usize = 10_000;
const MAX_TRANSCRIPT_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SshStatus {
    Disconnected,
    Connecting,
    Ready,
    Running,
    Failed,
}

impl SshStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SshLineKind {
    Command,
    Stdout,
    Stderr,
    Status,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SshLine {
    pub(crate) kind: SshLineKind,
    pub(crate) text: String,
}

pub(crate) struct SshState {
    pub(crate) target: Option<AgentIdentity>,
    pub(crate) session: Option<SshSession>,
    pub(crate) status: SshStatus,
    pub(crate) generation: u64,
    pub(crate) context_id: Option<TuiContextId>,
    pub(crate) input: String,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) history_cursor: Option<usize>,
    pub(crate) lines: Vec<SshLine>,
    transcript_bytes: usize,
    pub(crate) scroll: usize,
    pub(crate) content_height: Cell<usize>,
    pub(crate) viewport_height: Cell<usize>,
    pub(crate) error: Option<String>,
    pub(crate) cancel: Option<oneshot::Sender<()>>,
}

impl Default for SshState {
    fn default() -> Self {
        Self {
            target: None,
            session: None,
            status: SshStatus::Disconnected,
            generation: 0,
            context_id: None,
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_cursor: None,
            lines: Vec::new(),
            transcript_bytes: 0,
            scroll: 0,
            content_height: Cell::new(0),
            viewport_height: Cell::new(0),
            error: None,
            cancel: None,
        }
    }
}

impl SshState {
    pub(crate) fn reset(&mut self) {
        self.cancel_command();
        let generation = self.generation.wrapping_add(1);
        *self = Self::default();
        self.generation = generation;
    }

    pub(crate) fn begin_connect(&mut self, target: AgentIdentity) -> u64 {
        self.cancel_command();
        self.generation = self.generation.wrapping_add(1);
        self.target = Some(target);
        self.session = None;
        self.status = SshStatus::Connecting;
        self.context_id = None;
        self.input.clear();
        self.cursor = 0;
        self.history_cursor = None;
        self.lines.clear();
        self.transcript_bytes = 0;
        self.scroll = 0;
        self.error = None;
        self.cancel = None;
        self.generation
    }

    pub(crate) fn connected(&mut self, context_id: TuiContextId, session: SshSession) {
        let agent = session.agent().to_string();
        let tool = session.tool().to_string();
        self.context_id = Some(context_id);
        self.session = Some(session);
        self.status = SshStatus::Ready;
        self.error = None;
        self.push(
            SshLineKind::Status,
            format!("Connected to {agent} with `{tool}`"),
        );
    }

    pub(crate) fn failed(&mut self, message: String) {
        self.session = None;
        self.status = SshStatus::Failed;
        self.error = Some(message.clone());
        self.push(SshLineKind::Error, message);
    }

    pub(crate) fn begin_command(
        &mut self,
        script: String,
        cancel: oneshot::Sender<()>,
    ) -> Option<SshSession> {
        let session = self.session.take()?;
        self.status = SshStatus::Running;
        self.cancel = Some(cancel);
        self.error = None;
        self.history.push(script.clone());
        self.history_cursor = None;
        self.push(SshLineKind::Command, script);
        Some(session)
    }

    pub(crate) fn finish_command(
        &mut self,
        context_id: TuiContextId,
        session: SshSession,
        result: EmbeddedSshResult,
    ) {
        self.cancel = None;
        self.context_id = Some(context_id);
        self.session = Some(session);
        match result {
            EmbeddedSshResult::Completed {
                stdout,
                stderr,
                exit_code,
                elapsed,
                agent_replaced,
                ..
            } => {
                self.push_text(SshLineKind::Stdout, &stdout);
                self.push_text(SshLineKind::Stderr, &stderr);
                if agent_replaced {
                    self.push(
                        SshLineKind::Status,
                        "The Agent was deleted and recreated; this is a new Agent".to_string(),
                    );
                }
                self.push(
                    if exit_code == 0 {
                        SshLineKind::Status
                    } else {
                        SshLineKind::Error
                    },
                    format!("exit {exit_code} · {}", elapsed_label(elapsed)),
                );
                self.status = SshStatus::Ready;
                self.error = None;
            }
            EmbeddedSshResult::Failed {
                message,
                idempotency_key,
                elapsed,
                fatal,
                ..
            } => {
                self.push(SshLineKind::Error, message.clone());
                if fatal {
                    self.push(
                        SshLineKind::Status,
                        format!("Outcome lookup key {idempotency_key}"),
                    );
                    self.session = None;
                    self.status = SshStatus::Failed;
                } else {
                    self.status = SshStatus::Ready;
                }
                self.error = Some(format!("{} · {}", message, elapsed_label(elapsed)));
            }
            EmbeddedSshResult::Interrupted {
                message,
                idempotency_key,
                elapsed,
                command_running,
            } => {
                self.push(SshLineKind::Error, message.clone());
                if command_running {
                    self.push(
                        SshLineKind::Status,
                        format!("The command may still be running · lookup key {idempotency_key}"),
                    );
                }
                self.error = Some(format!("{} · {}", message, elapsed_label(elapsed)));
                self.status = SshStatus::Ready;
            }
        }
        self.follow_latest();
    }

    pub(crate) fn cwd(&self) -> &str {
        self.session.as_ref().map_or("", SshSession::cwd)
    }

    pub(crate) fn cancel_command(&mut self) -> bool {
        self.cancel
            .take()
            .is_some_and(|cancel| cancel.send(()).is_ok())
    }

    pub(crate) fn push(&mut self, kind: SshLineKind, text: String) {
        let text = truncate_utf8(text, MAX_LINE_BYTES);
        self.transcript_bytes = self.transcript_bytes.saturating_add(text.len());
        self.lines.push(SshLine { kind, text });
        while self.lines.len() > MAX_TRANSCRIPT_LINES
            || self.transcript_bytes > MAX_TRANSCRIPT_BYTES
        {
            let removed = self.lines.remove(0);
            self.transcript_bytes = self.transcript_bytes.saturating_sub(removed.text.len());
        }
        self.follow_latest();
    }

    fn push_text(&mut self, kind: SshLineKind, text: &str) {
        for line in text.lines() {
            self.push(kind, line.to_string());
        }
        if !text.is_empty() && text.lines().next().is_none() {
            self.push(kind, String::new());
        }
    }

    pub(crate) fn follow_latest(&mut self) {
        self.scroll = usize::MAX;
    }

    pub(crate) fn scroll_up(&mut self, amount: usize) {
        let maximum = self
            .content_height
            .get()
            .saturating_sub(self.viewport_height.get());
        self.scroll = self.scroll.min(maximum).saturating_sub(amount);
    }

    pub(crate) fn scroll_down(&mut self, amount: usize) {
        let maximum = self
            .content_height
            .get()
            .saturating_sub(self.viewport_height.get());
        self.scroll = self.scroll.saturating_add(amount).min(maximum);
    }
}

fn truncate_utf8(mut text: String, maximum: usize) -> String {
    if text.len() <= maximum {
        return text;
    }
    let mut end = maximum.saturating_sub('…'.len_utf8()).min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.truncate(end);
    text.push('…');
    text
}

fn elapsed_label(elapsed: Duration) -> String {
    if elapsed.as_secs() > 0 {
        format!("{:.2}s", elapsed.as_secs_f64())
    } else {
        format!("{}ms", elapsed.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_LINE_BYTES, SshLineKind, SshState, SshStatus, elapsed_label};
    use crate::tui::ops::AgentIdentity;
    use std::time::Duration;
    use test_r::test;

    #[test]
    fn elapsed_time_uses_compact_units() {
        assert_eq!(elapsed_label(Duration::from_millis(42)), "42ms");
        assert_eq!(elapsed_label(Duration::from_millis(1250)), "1.25s");
    }

    #[test]
    fn reconnect_invalidates_previous_work_and_clears_transcript() {
        let mut state = SshState::default();
        let first = state.begin_connect(AgentIdentity {
            component: "one".to_string(),
            agent_id: "First()".to_string(),
        });
        state.failed("not bound".to_string());

        let second = state.begin_connect(AgentIdentity {
            component: "two".to_string(),
            agent_id: "Second()".to_string(),
        });

        assert_ne!(first, second);
        assert_eq!(state.status, SshStatus::Connecting);
        assert_eq!(state.target.as_ref().unwrap().agent_id, "Second()");
        assert!(state.lines.is_empty());
        assert!(state.error.is_none());
    }

    #[test]
    fn transcript_lines_are_bounded_without_splitting_utf8() {
        let mut state = SshState::default();
        state.push(SshLineKind::Stdout, "é".repeat(MAX_LINE_BYTES));

        let line = &state.lines[0].text;
        assert!(line.len() <= MAX_LINE_BYTES);
        assert!(line.ends_with('…'));
    }

    #[test]
    async fn cancel_command_notifies_the_in_flight_request_once() {
        let mut state = SshState::default();
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        state.cancel = Some(cancel);

        assert!(state.cancel_command());
        assert!(cancelled.await.is_ok());
        assert!(!state.cancel_command());
    }
}
