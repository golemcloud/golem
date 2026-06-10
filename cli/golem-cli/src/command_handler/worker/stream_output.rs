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

use crate::command_handler::log::print_structured_document;
use crate::log::log_error;
use crate::model::agent::stream::AgentStreamEvent;
use crate::model::format::Format;
use crate::model::worker::AgentLogStreamOptions;
use colored::Colorize;
use golem_common::model::{IdempotencyKey, LogLevel, Timestamp};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite;

#[derive(Clone)]
pub struct WorkerStreamOutput {
    state: Arc<Mutex<WorkerStreamOutputState>>,
    options: AgentLogStreamOptions,
    format: Format,
}

struct WorkerStreamOutputState {
    pub last_stdout_timestamp: Timestamp,
    pub stdout: String,
    pub last_stderr_timestamp: Timestamp,
    pub stderr: String,
    pub last_timestamp: Timestamp,
    pub last_timestamp_hashes: HashSet<u64>,
}

impl WorkerStreamOutput {
    pub fn new(options: AgentLogStreamOptions, format: Format) -> Self {
        WorkerStreamOutput {
            state: Arc::new(Mutex::new(WorkerStreamOutputState {
                last_stdout_timestamp: Timestamp::now_utc(),
                stdout: String::new(),
                last_stderr_timestamp: Timestamp::now_utc(),
                stderr: String::new(),
                last_timestamp: Timestamp::from_str("2000-01-01T00:00:00Z").unwrap(),
                last_timestamp_hashes: HashSet::new(),
            })),
            options,
            format,
        }
    }

    pub async fn emit_stdout(&self, timestamp: Timestamp, message: String) {
        let mut state = self.state.lock().await;
        state.last_stdout_timestamp = timestamp;

        if !self
            .check_already_seen(&mut state, timestamp, &message)
            .await
        {
            let lines = message.lines().collect::<Vec<_>>();
            for (idx, line) in lines.iter().enumerate() {
                if idx == (lines.len() - 1) {
                    // last line, if message did not end with newline, just store it
                    if message.ends_with('\n') {
                        self.print_stdout(timestamp, &format!("{}{}", state.stdout, line));
                        state.stdout = String::new();
                    } else {
                        state.stdout = format!("{}{}", state.stdout, line);
                    }
                } else if idx == 0 {
                    // first line, there are more
                    self.print_stdout(timestamp, &format!("{}{}", state.stdout, line));
                    state.stdout = String::new();
                } else {
                    // middle line
                    self.print_stdout(timestamp, line);
                }
            }
        }
    }

    pub async fn emit_stderr(&self, timestamp: Timestamp, message: String) {
        let mut state = self.state.lock().await;
        state.last_stderr_timestamp = timestamp;

        if !self
            .check_already_seen(&mut state, timestamp, &message)
            .await
        {
            let lines = message.lines().collect::<Vec<_>>();
            for (idx, line) in lines.iter().enumerate() {
                if idx == (lines.len() - 1) {
                    // last line, if message did not end with newline, just store it
                    if message.ends_with('\n') {
                        self.print_stderr(timestamp, &format!("{}{}", state.stderr, line));
                        state.stderr = String::new();
                    } else {
                        state.stderr = format!("{}{}", state.stderr, line);
                    }
                } else if idx == 0 {
                    // first line, there are more
                    self.print_stderr(timestamp, &format!("{}{}", state.stderr, line));
                    state.stderr = String::new();
                } else {
                    // middle line
                    self.print_stderr(timestamp, line);
                }
            }
        }
    }

    pub async fn emit_log(
        &self,
        timestamp: Timestamp,
        level: LogLevel,
        context: String,
        message: String,
    ) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(&mut state, timestamp, &message)
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
            ));
        }
    }

    pub async fn emit_stream_closed(&self, timestamp: Timestamp) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(&mut state, timestamp, "Stream closed")
            .await
            && !self.options.logs_only
        {
            self.output_event(AgentStreamEvent::stream_closed(timestamp));
        }
    }

    pub async fn emit_stream_error(&self, timestamp: Timestamp, error: tungstenite::error::Error) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(&mut state, timestamp, "Stream error")
            .await
            && !self.options.logs_only
        {
            self.output_event(AgentStreamEvent::stream_error(timestamp, error));
        }
    }

    pub async fn emit_invocation_start(
        &self,
        timestamp: Timestamp,
        function_name: String,
        idempotency_key: IdempotencyKey,
    ) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(
                &mut state,
                timestamp,
                &format!("{function_name} {idempotency_key} started"),
            )
            .await
            && !self.options.logs_only
        {
            self.output_event(AgentStreamEvent::invocation_started(
                timestamp,
                function_name.clone(),
                idempotency_key.clone(),
            ));
        }
    }

    pub async fn emit_invocation_finished(
        &self,
        timestamp: Timestamp,
        function_name: String,
        idempotency_key: IdempotencyKey,
    ) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(
                &mut state,
                timestamp,
                &format!("{function_name} {idempotency_key} finished"),
            )
            .await
            && !self.options.logs_only
        {
            self.output_event(AgentStreamEvent::invocation_finished(
                timestamp,
                function_name.clone(),
                idempotency_key.clone(),
            ));
        }
    }

    pub async fn emit_missed_messages(&self, timestamp: Timestamp, number_of_missed_messages: u64) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(
                &mut state,
                timestamp,
                &format!("{number_of_missed_messages} messages missed"),
            )
            .await
            && !self.options.logs_only
        {
            self.output_event(AgentStreamEvent::missed_messages(
                timestamp,
                number_of_missed_messages,
            ));
        }
    }

    pub async fn flush(&self) {
        let mut state = self.state.lock().await;
        if !state.stdout.is_empty() {
            self.print_stdout(state.last_stdout_timestamp, &state.stdout);
            state.stdout = String::new();
        }
        if !state.stderr.is_empty() {
            self.print_stderr(state.last_stdout_timestamp, &state.stderr);
            state.stderr = String::new();
        }
    }

    async fn check_already_seen(
        &self,
        state: &mut WorkerStreamOutputState,
        timestamp: Timestamp,
        message: &str,
    ) -> bool {
        let mut hasher = DefaultHasher::new();
        message.hash(&mut hasher);
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

    fn print_stdout(&self, timestamp: Timestamp, message: &str) {
        self.output_event(AgentStreamEvent::stdout(timestamp, message));
    }

    fn print_stderr(&self, timestamp: Timestamp, message: &str) {
        self.output_event(AgentStreamEvent::stderr(timestamp, message));
    }

    fn output_event(&self, event: AgentStreamEvent) {
        if self.format.is_structured() {
            self.machine_event(&event);
        } else {
            self.colored(event.text_log_level(), &event.render_text(&self.options));
        }
    }

    fn machine_event(&self, event: &AgentStreamEvent) {
        // Stream events are flat structures, so rendering them cannot
        // realistically fail; as this is called from the websocket read loop,
        // errors are logged instead of being propagated
        if let Err(error) = print_structured_document(self.format, self.options.colors, event) {
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
            println!("{colored}");
        } else {
            println!("{s}");
        }
    }
}
