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

use crate::command_handler::log::print_command_output_document;
use crate::log::{log_error, print_line};
use crate::model::agent::AgentLogStreamOptions;
use crate::model::agent::stream::AgentStreamEvent;
use crate::model::format::Format;
use colored::Colorize;
use golem_common::model::{LogLevel, Timestamp};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite;

/// Splits a write into complete lines, each with its invocation ID. A line split across several
/// writes of one invocation keeps the invocation ID of the write that started it; a pending line
/// of another invocation is completed first. An unterminated last line stays in `pending`.
fn complete_lines(
    pending: &mut String,
    pending_invocation_id: &mut Option<String>,
    message: &str,
    invocation_id: Option<String>,
) -> Vec<(String, Option<String>)> {
    let mut complete = Vec::new();
    if !pending.is_empty() && *pending_invocation_id != invocation_id {
        complete.push((std::mem::take(pending), pending_invocation_id.take()));
    }
    let continues_pending = !pending.is_empty();
    let lines = message.lines().collect::<Vec<_>>();
    for (idx, line) in lines.iter().enumerate() {
        let (text, id) = if idx == 0 && continues_pending {
            (
                format!("{}{line}", std::mem::take(pending)),
                pending_invocation_id.take(),
            )
        } else {
            (line.to_string(), invocation_id.clone())
        };
        if idx == lines.len() - 1 && !message.ends_with('\n') {
            *pending = text;
            *pending_invocation_id = id;
        } else {
            complete.push((text, id));
        }
    }
    complete
}

#[derive(Clone)]
pub struct AgentStreamOutput {
    state: Arc<Mutex<AgentStreamOutputState>>,
    options: AgentLogStreamOptions,
    format: Format,
}

struct AgentStreamOutputState {
    pub last_stdout_timestamp: Timestamp,
    pub stdout: String,
    pub stdout_invocation_id: Option<String>,
    pub last_stderr_timestamp: Timestamp,
    pub stderr: String,
    pub stderr_invocation_id: Option<String>,
    pub last_timestamp: Timestamp,
    pub last_timestamp_hashes: HashSet<u64>,
}

impl AgentStreamOutput {
    pub fn new(options: AgentLogStreamOptions, format: Format) -> Self {
        AgentStreamOutput {
            state: Arc::new(Mutex::new(AgentStreamOutputState {
                last_stdout_timestamp: Timestamp::now_utc(),
                stdout: String::new(),
                stdout_invocation_id: None,
                last_stderr_timestamp: Timestamp::now_utc(),
                stderr: String::new(),
                stderr_invocation_id: None,
                last_timestamp: Timestamp::from_str("2000-01-01T00:00:00Z").unwrap(),
                last_timestamp_hashes: HashSet::new(),
            })),
            options,
            format,
        }
    }

    /// A line split across several writes keeps the invocation ID of the write that started it.
    pub async fn emit_stdout(
        &self,
        timestamp: Timestamp,
        message: String,
        invocation_id: Option<String>,
    ) {
        let mut state = self.state.lock().await;
        state.last_stdout_timestamp = timestamp;

        if !self
            .check_already_seen(&mut state, timestamp, ("stdout", &message, &invocation_id))
            .await
        {
            let state = &mut *state;
            for (line, invocation_id) in complete_lines(
                &mut state.stdout,
                &mut state.stdout_invocation_id,
                &message,
                invocation_id,
            ) {
                self.print_stdout(timestamp, &line, invocation_id);
            }
        }
    }

    /// A line split across several writes keeps the invocation ID of the write that started it.
    pub async fn emit_stderr(
        &self,
        timestamp: Timestamp,
        message: String,
        invocation_id: Option<String>,
    ) {
        let mut state = self.state.lock().await;
        state.last_stderr_timestamp = timestamp;

        if !self
            .check_already_seen(&mut state, timestamp, ("stderr", &message, &invocation_id))
            .await
        {
            let state = &mut *state;
            for (line, invocation_id) in complete_lines(
                &mut state.stderr,
                &mut state.stderr_invocation_id,
                &message,
                invocation_id,
            ) {
                self.print_stderr(timestamp, &line, invocation_id);
            }
        }
    }

    pub async fn emit_log(
        &self,
        timestamp: Timestamp,
        level: LogLevel,
        context: String,
        message: String,
        invocation_id: Option<String>,
    ) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(&mut state, timestamp, ("log", &message, &invocation_id))
            .await
        {
            let level_str = match level {
                LogLevel::Trace => "TRACE",
                LogLevel::Debug => "DEBUG",
                LogLevel::Info => "INFO",
                LogLevel::Warn => "WARN",
                LogLevel::Error => "ERROR",
                LogLevel::Critical => "CRITICAL",
            };

            self.output_event(AgentStreamEvent::log(
                timestamp,
                level_str,
                context.clone(),
                message.clone(),
                invocation_id,
            ));
        }
    }

    /// Connection notices are stamped with the local clock, so they stay out of deduplication,
    /// which compares the server timestamps of agent events.
    pub async fn emit_stream_closed(&self, timestamp: Timestamp) {
        if !self.options.logs_only {
            self.output_event(AgentStreamEvent::stream_closed(timestamp));
        }
    }

    pub async fn emit_stream_error(&self, timestamp: Timestamp, error: tungstenite::error::Error) {
        if !self.options.logs_only {
            self.output_event(AgentStreamEvent::stream_error(timestamp, error));
        }
    }

    pub async fn emit_invocation_start(
        &self,
        timestamp: Timestamp,
        function_name: String,
        invocation_id: String,
    ) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(
                &mut state,
                timestamp,
                &format!("{function_name} {invocation_id} started"),
            )
            .await
            && !self.options.logs_only
        {
            self.output_event(AgentStreamEvent::invocation_started(
                timestamp,
                function_name.clone(),
                invocation_id.clone(),
            ));
        }
    }

    pub async fn emit_invocation_finished(
        &self,
        timestamp: Timestamp,
        function_name: String,
        invocation_id: String,
    ) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(
                &mut state,
                timestamp,
                &format!("{function_name} {invocation_id} finished"),
            )
            .await
            && !self.options.logs_only
        {
            self.output_event(AgentStreamEvent::invocation_finished(
                timestamp,
                function_name.clone(),
                invocation_id.clone(),
            ));
        }
    }

    /// A lag notice is stamped with the local clock too, so it also stays out of deduplication.
    pub async fn emit_missed_messages(&self, timestamp: Timestamp, number_of_missed_messages: u64) {
        if !self.options.logs_only {
            self.output_event(AgentStreamEvent::missed_messages(
                timestamp,
                number_of_missed_messages,
            ));
        }
    }

    pub async fn flush(&self) {
        let mut state = self.state.lock().await;
        if !state.stdout.is_empty() {
            let invocation_id = state.stdout_invocation_id.take();
            self.print_stdout(state.last_stdout_timestamp, &state.stdout, invocation_id);
            state.stdout = String::new();
        }
        if !state.stderr.is_empty() {
            let invocation_id = state.stderr_invocation_id.take();
            self.print_stderr(state.last_stderr_timestamp, &state.stderr, invocation_id);
            state.stderr = String::new();
        }
    }

    async fn check_already_seen(
        &self,
        state: &mut AgentStreamOutputState,
        timestamp: Timestamp,
        key: impl Hash,
    ) -> bool {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let hash = hasher.finish();

        match state.last_timestamp.cmp(&timestamp) {
            Ordering::Less => {
                // definitely new
                state.last_timestamp = timestamp;
                state.last_timestamp_hashes.clear();
                state.last_timestamp_hashes.insert(hash);
                false
            }
            Ordering::Equal => {
                if state.last_timestamp_hashes.contains(&hash) {
                    // old
                    true
                } else {
                    // new
                    state.last_timestamp_hashes.insert(hash);
                    false
                }
            }
            Ordering::Greater => {
                // definitely old
                true
            }
        }
    }

    fn print_stdout(&self, timestamp: Timestamp, message: &str, invocation_id: Option<String>) {
        self.output_event(AgentStreamEvent::stdout(timestamp, message, invocation_id));
    }

    fn print_stderr(&self, timestamp: Timestamp, message: &str, invocation_id: Option<String>) {
        self.output_event(AgentStreamEvent::stderr(timestamp, message, invocation_id));
    }

    fn output_event(&self, event: AgentStreamEvent) {
        if self.format.is_structured() {
            self.machine_event(event);
        } else {
            self.colored(event.text_log_level(), &event.render_text(&self.options));
        }
    }

    fn machine_event(&self, event: AgentStreamEvent) {
        // Stream events are flat structures, so rendering them cannot
        // realistically fail; as this is called from the websocket read loop,
        // errors are logged instead of being propagated
        if let Err(error) = print_command_output_document(self.format, self.options.colors, event) {
            log_error(format!("Failed to render agent stream event: {error:#}"));
        }
    }

    fn colored(&self, level: LogLevel, s: &str) {
        if self.options.colors {
            let colored = match level {
                LogLevel::Trace => s.blue(),
                LogLevel::Debug => s.green(),
                LogLevel::Info => s.white(),
                LogLevel::Warn => s.yellow(),
                LogLevel::Error => s.red(),
                LogLevel::Critical => s.red().bold(),
            };
            print_line(colored);
        } else {
            print_line(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::complete_lines;
    use test_r::test;

    #[test]
    fn complete_lines_attribute_each_line_to_the_invocation_that_wrote_it() {
        let (a, b) = (Some("a".to_string()), Some("b".to_string()));
        let mut pending = String::new();
        let mut pending_id = None;
        let mut write = |message: &str, id: &Option<String>| {
            complete_lines(&mut pending, &mut pending_id, message, id.clone())
        };

        assert_eq!(
            write("one\ntwo\nthree\n", &a),
            vec![
                ("one".to_string(), a.clone()),
                ("two".to_string(), a.clone()),
                ("three".to_string(), a.clone())
            ]
        );
        assert_eq!(write("par", &a), vec![]);
        assert_eq!(
            write("tial\nnext", &a),
            vec![("partial".to_string(), a.clone())]
        );
        assert_eq!(
            write("done\n", &b),
            vec![
                ("next".to_string(), a.clone()),
                ("done".to_string(), b.clone())
            ]
        );
        assert_eq!(write("init ", &None), vec![]);
        assert_eq!(
            write("done\nfirst ", &None),
            vec![("init done".to_string(), None)]
        );
        assert_eq!(
            write("call\n", &a),
            vec![
                ("first ".to_string(), None),
                ("call".to_string(), a.clone())
            ]
        );
    }
}
