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

use crate::command_handler::log::print_toon_document;
use crate::model::format::Format;
use crate::model::text::fmt::{to_colored_json, to_colored_yaml};
use crate::model::worker::AgentLogStreamOptions;
use colored::Colorize;
use golem_common::model::{IdempotencyKey, LogLevel, Timestamp};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt::Write;
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

fn stream_event(
    timestamp: Timestamp,
    kind: &str,
    level: &str,
    context: &str,
    message: &str,
    extra_fields: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let mut event = serde_json::Map::new();
    event.insert("timestamp".to_string(), serde_json::json!(timestamp));
    event.insert("kind".to_string(), serde_json::json!(kind));
    event.insert("level".to_string(), serde_json::json!(level));
    event.insert("context".to_string(), serde_json::json!(context));
    event.insert("message".to_string(), serde_json::json!(message));

    for (key, value) in extra_fields {
        event.insert(key, value);
    }

    serde_json::Value::Object(event)
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

            match self.format {
                Format::Json
                | Format::PrettyJson
                | Format::Yaml
                | Format::PrettyYaml
                | Format::Toon => self.machine_event(stream_event(
                    timestamp,
                    "log",
                    level_str,
                    &context,
                    &message,
                    serde_json::Map::new(),
                )),
                Format::Text => {
                    let prefix = self.prefix(timestamp, level_str);
                    self.colored(level, &format!("{prefix}[{context}] {message}"));
                }
            }
        }
    }

    pub async fn emit_stream_closed(&self, timestamp: Timestamp) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(&mut state, timestamp, "Stream closed")
            .await
            && !self.options.logs_only
        {
            match self.format {
                Format::Json
                | Format::PrettyJson
                | Format::Yaml
                | Format::PrettyYaml
                | Format::Toon => self.machine_event(stream_event(
                    timestamp,
                    "stream-closed",
                    "STREAM",
                    "",
                    "Stream closed",
                    serde_json::Map::new(),
                )),
                Format::Text => {
                    let prefix = self.prefix(timestamp, "STREAM");
                    self.colored(LogLevel::Debug, &format!("{prefix}Stream closed"));
                }
            }
        }
    }

    pub async fn emit_stream_error(&self, timestamp: Timestamp, error: tungstenite::error::Error) {
        let mut state = self.state.lock().await;

        if !self
            .check_already_seen(&mut state, timestamp, "Stream error")
            .await
            && !self.options.logs_only
        {
            match self.format {
                Format::Json
                | Format::PrettyJson
                | Format::Yaml
                | Format::PrettyYaml
                | Format::Toon => {
                    let mut fields = serde_json::Map::new();
                    fields.insert("error".to_string(), serde_json::json!(error.to_string()));
                    self.machine_event(stream_event(
                        timestamp,
                        "stream-error",
                        "WARN",
                        "",
                        &format!("Stream failed with error: {error}"),
                        fields,
                    ));
                }
                Format::Text => {
                    let prefix = self.prefix(timestamp, "STREAM");
                    self.colored(
                        LogLevel::Warn,
                        &format!("{prefix}Stream failed with error: {error}"),
                    );
                }
            }
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
            match self.format {
                Format::Json
                | Format::PrettyJson
                | Format::Yaml
                | Format::PrettyYaml
                | Format::Toon => {
                    let mut fields = serde_json::Map::new();
                    fields.insert("functionName".to_string(), serde_json::json!(function_name));
                    fields.insert(
                        "idempotencyKey".to_string(),
                        serde_json::json!(idempotency_key.to_string()),
                    );
                    self.machine_event(stream_event(
                        timestamp,
                        "invocation-started",
                        "TRACE",
                        "",
                        "Invocation started",
                        fields,
                    ));
                }
                Format::Text => {
                    let prefix = self.prefix(timestamp, "INVOKE");
                    self.colored(
                        LogLevel::Trace,
                        &format!("{prefix}STARTED  {function_name} ({idempotency_key})"),
                    );
                }
            }
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
            match self.format {
                Format::Json
                | Format::PrettyJson
                | Format::Yaml
                | Format::PrettyYaml
                | Format::Toon => {
                    let mut fields = serde_json::Map::new();
                    fields.insert("functionName".to_string(), serde_json::json!(function_name));
                    fields.insert(
                        "idempotencyKey".to_string(),
                        serde_json::json!(idempotency_key.to_string()),
                    );
                    self.machine_event(stream_event(
                        timestamp,
                        "invocation-finished",
                        "TRACE",
                        "",
                        "Invocation finished",
                        fields,
                    ));
                }
                Format::Text => {
                    let prefix = self.prefix(timestamp, "INVOKE");
                    self.colored(
                        LogLevel::Trace,
                        &format!("{prefix}FINISHED {function_name} ({idempotency_key})",),
                    );
                }
            }
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
            match self.format {
                Format::Json
                | Format::PrettyJson
                | Format::Yaml
                | Format::PrettyYaml
                | Format::Toon => {
                    let mut fields = serde_json::Map::new();
                    fields.insert(
                        "numberOfMissedMessages".to_string(),
                        serde_json::json!(number_of_missed_messages),
                    );
                    self.machine_event(stream_event(
                        timestamp,
                        "missed-messages",
                        "WARN",
                        "",
                        &format!(
                            "Stream output fell behind the server and {number_of_missed_messages} messages were missed"
                        ),
                        fields,
                    ));
                }
                Format::Text => {
                    let prefix = self.prefix(timestamp, "STREAM");
                    self.colored(
                        LogLevel::Warn,
                        &format!("{prefix}Stream output fell behind the server and {number_of_missed_messages} messages were missed", ),
                    );
                }
            }
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
        match self.format {
            Format::Json
            | Format::PrettyJson
            | Format::Yaml
            | Format::PrettyYaml
            | Format::Toon => self.machine_event(stream_event(
                timestamp,
                "stdout",
                "STDOUT",
                "",
                message,
                serde_json::Map::new(),
            )),
            Format::Text => {
                let prefix = self.prefix(timestamp, "STDOUT");
                self.colored(LogLevel::Info, &format!("{prefix}{message}"));
            }
        }
    }

    fn print_stderr(&self, timestamp: Timestamp, message: &str) {
        match self.format {
            Format::Json
            | Format::PrettyJson
            | Format::Yaml
            | Format::PrettyYaml
            | Format::Toon => self.machine_event(stream_event(
                timestamp,
                "stderr",
                "STDERR",
                "",
                message,
                serde_json::Map::new(),
            )),
            Format::Text => {
                let prefix = self.prefix(timestamp, "STDERR");
                self.colored(LogLevel::Error, &format!("{prefix}{message}"));
            }
        }
    }

    fn machine_event(&self, event: serde_json::Value) {
        match self.format {
            Format::Json => println!("{event}"),
            Format::PrettyJson => {
                if self.options.colors {
                    println!("{}", to_colored_json(&event).unwrap());
                } else {
                    println!("{}", serde_json::to_string_pretty(&event).unwrap());
                }
            }
            Format::Yaml => println!("---\n{}", serde_yaml::to_string(&event).unwrap()),
            Format::PrettyYaml => {
                if self.options.colors {
                    println!("---\n{}", to_colored_yaml(&event).unwrap());
                } else {
                    println!("---\n{}", serde_yaml::to_string(&event).unwrap());
                }
            }
            Format::Toon => print_toon_document(&event),
            Format::Text => unreachable!(),
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

    fn prefix(&self, timestamp: Timestamp, level_or_source: &str) -> String {
        let mut result = String::new();
        if self.options.show_timestamp {
            let _ = write!(&mut result, "[{timestamp}] ");
        }
        if self.options.show_level {
            let _ = result.write_char('[');
            let _ = result.write_str(level_or_source);
            for _ in level_or_source.len()..8 {
                let _ = result.write_char(' ');
            }
            let _ = result.write_char(']');
            let _ = result.write_char(' ');
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::stream_event;
    use golem_common::model::Timestamp;
    use std::str::FromStr;

    #[test_r::test]
    fn stream_event_contains_common_and_extra_fields() {
        let timestamp = Timestamp::from_str("2026-01-01T00:00:00Z").unwrap();
        let mut extra = serde_json::Map::new();
        extra.insert("functionName".to_string(), serde_json::json!("run"));

        let event = stream_event(
            timestamp,
            "invocation-started",
            "TRACE",
            "",
            "Invocation started",
            extra,
        );

        assert_eq!(event["kind"], "invocation-started");
        assert_eq!(event["level"], "TRACE");
        assert_eq!(event["message"], "Invocation started");
        assert_eq!(event["functionName"], "run");
    }
}
