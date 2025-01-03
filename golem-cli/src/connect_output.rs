// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::command::worker::WorkerConnectOptions;
use crate::model::Format;
use colored::Colorize;
use golem_common::model::{LogLevel, Timestamp};
use std::fmt::Write;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct ConnectOutput {
    state: Arc<Mutex<ConnectOutputState>>,
    options: WorkerConnectOptions,
    format: Format,
}

struct ConnectOutputState {
    pub last_stdout_timestamp: Timestamp,
    pub stdout: String,
    pub last_stderr_timestamp: Timestamp,
    pub stderr: String,
}

impl ConnectOutput {
    pub fn new(options: WorkerConnectOptions, format: Format) -> Self {
        ConnectOutput {
            state: Arc::new(Mutex::new(ConnectOutputState {
                last_stdout_timestamp: Timestamp::now_utc(),
                stdout: String::new(),
                last_stderr_timestamp: Timestamp::now_utc(),
                stderr: String::new(),
            })),
            options,
            format,
        }
    }

    pub async fn emit_stdout(&self, timestamp: Timestamp, message: String) {
        let mut state = self.state.lock().await;
        state.last_stdout_timestamp = timestamp;

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

    pub async fn emit_stderr(&self, timestamp: Timestamp, message: String) {
        let mut state = self.state.lock().await;
        state.last_stderr_timestamp = timestamp;

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

    pub fn emit_log(
        &self,
        timestamp: Timestamp,
        level: LogLevel,
        context: String,
        message: String,
    ) {
        let level_str = match level {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Critical => "CRITICAL",
        };

        match self.format {
            Format::Json => self.json(level_str, &context, &message),
            Format::Yaml => self.yaml(level_str, &context, &message),
            Format::Text => {
                let prefix = self.prefix(timestamp, level_str);
                self.colored(level, &format!("{prefix}[{context}] {message}"));
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

    fn print_stdout(&self, timestamp: Timestamp, message: &str) {
        match self.format {
            Format::Json => self.json("STDOUT", "", message),
            Format::Yaml => self.yaml("STDOUT", "", message),
            Format::Text => {
                let prefix = self.prefix(timestamp, "STDOUT");
                self.colored(LogLevel::Info, &format!("{prefix}{message}"));
            }
        }
    }

    fn print_stderr(&self, timestamp: Timestamp, message: &str) {
        match self.format {
            Format::Json => self.json("STDERR", "", message),
            Format::Yaml => self.yaml("STDERR", "", message),
            Format::Text => {
                let prefix = self.prefix(timestamp, "STDERR");
                self.colored(LogLevel::Error, &format!("{prefix}{message}"));
            }
        }
    }

    fn json(&self, level_or_source: &str, context: &str, message: &str) {
        let json = self.json_value(level_or_source, context, message);
        println!("{}", json);
    }

    fn yaml(&self, level_or_source: &str, context: &str, message: &str) {
        let json = self.json_value(level_or_source, context, message);
        println!("{}", serde_yaml::to_string(&json).unwrap());
    }

    fn json_value(&self, level_or_source: &str, context: &str, message: &str) -> serde_json::Value {
        serde_json::json!({
            "timestamp": Timestamp::now_utc(),
            "level": level_or_source,
            "context": context,
            "message": message,
        })
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
            println!("{}", colored);
        } else {
            println!("{}", s);
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
