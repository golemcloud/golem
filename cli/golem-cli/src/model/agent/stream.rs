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

use crate::model::text::fmt::format_stderr;
use crate::model::worker::AgentLogStreamOptions;
use golem_common::model::{IdempotencyKey, LogLevel, Timestamp};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStreamEvent {
    pub timestamp: Timestamp,
    pub kind: AgentStreamEventKind,
    pub level: String,
    pub context: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_of_missed_messages: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStreamEventKind {
    Log,
    Stdout,
    Stderr,
    StreamClosed,
    StreamError,
    InvocationStarted,
    InvocationFinished,
    MissedMessages,
}

impl AgentStreamEvent {
    pub fn new(
        timestamp: Timestamp,
        kind: AgentStreamEventKind,
        level: impl Into<String>,
        context: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            timestamp,
            kind,
            level: level.into(),
            context: context.into(),
            message: message.into(),
            function_name: None,
            idempotency_key: None,
            number_of_missed_messages: None,
            error: None,
        }
    }

    pub fn log(timestamp: Timestamp, level: &str, context: String, message: String) -> Self {
        Self::new(
            timestamp,
            AgentStreamEventKind::Log,
            level,
            context,
            message,
        )
    }

    pub fn stdout(timestamp: Timestamp, message: &str) -> Self {
        Self::new(
            timestamp,
            AgentStreamEventKind::Stdout,
            "STDOUT",
            "",
            message,
        )
    }

    pub fn stderr(timestamp: Timestamp, message: &str) -> Self {
        Self::new(
            timestamp,
            AgentStreamEventKind::Stderr,
            "STDERR",
            "",
            message,
        )
    }

    pub fn stream_closed(timestamp: Timestamp) -> Self {
        Self::new(
            timestamp,
            AgentStreamEventKind::StreamClosed,
            "STREAM",
            "",
            "Stream closed",
        )
    }

    pub fn stream_error(timestamp: Timestamp, error: impl ToString) -> Self {
        let error = error.to_string();
        let mut event = Self::new(
            timestamp,
            AgentStreamEventKind::StreamError,
            "WARN",
            "",
            format!("Stream failed with error: {error}"),
        );
        event.error = Some(error);
        event
    }

    pub fn invocation_started(
        timestamp: Timestamp,
        function_name: String,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        let mut event = Self::new(
            timestamp,
            AgentStreamEventKind::InvocationStarted,
            "TRACE",
            "",
            "Invocation started",
        );
        event.function_name = Some(function_name);
        event.idempotency_key = Some(idempotency_key.to_string());
        event
    }

    pub fn invocation_finished(
        timestamp: Timestamp,
        function_name: String,
        idempotency_key: IdempotencyKey,
    ) -> Self {
        let mut event = Self::new(
            timestamp,
            AgentStreamEventKind::InvocationFinished,
            "TRACE",
            "",
            "Invocation finished",
        );
        event.function_name = Some(function_name);
        event.idempotency_key = Some(idempotency_key.to_string());
        event
    }

    pub fn missed_messages(timestamp: Timestamp, number_of_missed_messages: u64) -> Self {
        let mut event = Self::new(
            timestamp,
            AgentStreamEventKind::MissedMessages,
            "WARN",
            "",
            format!(
                "Stream output fell behind the server and {number_of_missed_messages} messages were missed"
            ),
        );
        event.number_of_missed_messages = Some(number_of_missed_messages);
        event
    }

    pub fn text_log_level(&self) -> LogLevel {
        match self.kind {
            AgentStreamEventKind::Log => match self.level.as_str() {
                "TRACE" => LogLevel::Trace,
                "DEBUG" => LogLevel::Debug,
                "INFO" => LogLevel::Info,
                "WARN" => LogLevel::Warn,
                "ERROR" => LogLevel::Error,
                "CRITICAL" => LogLevel::Critical,
                _ => LogLevel::Info,
            },
            AgentStreamEventKind::Stdout => LogLevel::Info,
            AgentStreamEventKind::Stderr => LogLevel::Error,
            AgentStreamEventKind::StreamClosed => LogLevel::Debug,
            AgentStreamEventKind::StreamError | AgentStreamEventKind::MissedMessages => {
                LogLevel::Warn
            }
            AgentStreamEventKind::InvocationStarted | AgentStreamEventKind::InvocationFinished => {
                LogLevel::Trace
            }
        }
    }

    pub fn render_text(&self, options: &AgentLogStreamOptions) -> String {
        let mut result = String::new();
        if options.show_timestamp {
            result.push_str(&format!("[{}] ", self.timestamp));
        }
        if options.show_level {
            let level = self.text_level_label();
            result.push('[');
            result.push_str(level);
            for _ in level.len()..8 {
                result.push(' ');
            }
            result.push_str("] ");
        }
        if options.colors && matches!(self.kind, AgentStreamEventKind::Stderr) {
            result.push_str(&format_stderr(&self.text_message()))
        } else {
            result.push_str(&self.text_message());
        }
        result
    }

    fn text_level_label(&self) -> &str {
        match self.kind {
            AgentStreamEventKind::Log => &self.level,
            AgentStreamEventKind::Stdout => "STDOUT",
            AgentStreamEventKind::Stderr => "STDERR",
            AgentStreamEventKind::StreamClosed
            | AgentStreamEventKind::StreamError
            | AgentStreamEventKind::MissedMessages => "STREAM",
            AgentStreamEventKind::InvocationStarted | AgentStreamEventKind::InvocationFinished => {
                "INVOKE"
            }
        }
    }

    fn text_message(&self) -> String {
        match self.kind {
            AgentStreamEventKind::Log => format!("[{}] {}", self.context, self.message),
            AgentStreamEventKind::Stdout
            | AgentStreamEventKind::Stderr
            | AgentStreamEventKind::StreamClosed
            | AgentStreamEventKind::StreamError
            | AgentStreamEventKind::MissedMessages => self.message.clone(),
            AgentStreamEventKind::InvocationStarted => format!(
                "STARTED  {} ({})",
                self.function_name.as_deref().unwrap_or_default(),
                self.idempotency_key.as_deref().unwrap_or_default()
            ),
            AgentStreamEventKind::InvocationFinished => format!(
                "FINISHED {} ({})",
                self.function_name.as_deref().unwrap_or_default(),
                self.idempotency_key.as_deref().unwrap_or_default()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentStreamEvent, AgentStreamEventKind};
    use crate::model::worker::AgentLogStreamOptions;
    use golem_common::model::{IdempotencyKey, Timestamp};
    use std::str::FromStr;
    use test_r::test;

    #[test]
    fn agent_stream_event_contains_common_and_extra_fields() {
        let timestamp = Timestamp::from_str("2026-01-01T00:00:00Z").unwrap();
        let event = AgentStreamEvent::new(
            timestamp,
            AgentStreamEventKind::InvocationStarted,
            "TRACE",
            "",
            "Invocation started",
        );
        let event = AgentStreamEvent {
            function_name: Some("run".to_string()),
            ..event
        };
        let event = serde_json::to_value(event).unwrap();

        assert_eq!(event["kind"], "invocation-started");
        assert_eq!(event["level"], "TRACE");
        assert_eq!(event["message"], "Invocation started");
        assert_eq!(event["functionName"], "run");
    }

    #[test]
    fn agent_stream_event_renders_text_with_prefix() {
        let timestamp = Timestamp::from_str("2026-01-01T00:00:00Z").unwrap();
        let event = AgentStreamEvent::stdout(timestamp, "hello");

        assert_eq!(
            event.render_text(&AgentLogStreamOptions {
                colors: false,
                show_timestamp: true,
                show_level: true,
                logs_only: false,
            }),
            "[2026-01-01T00:00:00.000Z] [STDOUT  ] hello"
        );
    }

    #[test]
    fn agent_stream_event_renders_invocation_text() {
        let timestamp = Timestamp::from_str("2026-01-01T00:00:00Z").unwrap();
        let idempotency_key = IdempotencyKey::fresh();
        let rendered_idempotency_key = idempotency_key.to_string();
        let event =
            AgentStreamEvent::invocation_started(timestamp, "run".to_string(), idempotency_key);

        assert_eq!(
            event.render_text(&AgentLogStreamOptions {
                colors: false,
                show_timestamp: false,
                show_level: true,
                logs_only: false,
            }),
            format!("[INVOKE  ] STARTED  run ({rendered_idempotency_key})")
        );
    }
}
