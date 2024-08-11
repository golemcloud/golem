// Copyright 2024 Golem Cloud
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

use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct ConnectOutput {
    state: Arc<Mutex<ConnectOutputState>>,
}

struct ConnectOutputState {
    pub stdout: String,
    pub stderr: String,
}

// TODO: configurable prefix (source, level, worker name, timestamp), use colors

impl ConnectOutput {
    pub fn new() -> Self {
        ConnectOutput {
            state: Arc::new(Mutex::new(ConnectOutputState {
                stdout: String::new(),
                stderr: String::new(),
            })),
        }
    }

    pub async fn emit_stdout(&self, message: String) {
        let mut state = self.state.lock().await;
        let lines = message.lines().collect::<Vec<_>>();
        for (idx, line) in lines.iter().enumerate() {
            if idx == (lines.len() - 1) {
                // last line, if message did not end with newline, just store it
                if message.ends_with('\n') {
                    self.print_stdout(&format!("{}{}", state.stdout, line));
                    state.stdout = String::new();
                } else {
                    state.stdout = format!("{}{}", state.stdout, line);
                }
            } else if idx == 0 {
                // first line, there are more
                self.print_stdout(&format!("{}{}", state.stdout, line));
                state.stdout = String::new();
            } else {
                // middle line
                self.print_stdout(line);
            }
        }
    }

    pub async fn emit_stderr(&self, message: String) {
        let mut state = self.state.lock().await;
        let lines = message.lines().collect::<Vec<_>>();
        for (idx, line) in lines.iter().enumerate() {
            if idx == (lines.len() - 1) {
                // last line, if message did not end with newline, just store it
                if message.ends_with('\n') {
                    self.print_stdout(&format!("{}{}", state.stderr, line));
                    state.stderr = String::new();
                } else {
                    state.stderr = format!("{}{}", state.stderr, line);
                }
            } else if idx == 0 {
                // first line, there are more
                self.print_stderr(&format!("{}{}", state.stderr, line));
                state.stderr = String::new();
            } else {
                // middle line
                self.print_stderr(line);
            }
        }
    }

    pub fn emit_log(&self, level: i32, context: String, message: String) {
        let level = match level {
            0 => "TRACE   ",
            1 => "DEBUG   ",
            2 => "INFO    ",
            3 => "WARN    ",
            4 => "ERROR   ",
            5 => "CRITICAL",
            _ => "        ",
        };
        println!("[{level}] [{context}] {message}");
    }

    pub async fn flush(&self) {
        let mut state = self.state.lock().await;
        if !state.stdout.is_empty() {
            self.print_stdout(&state.stdout);
            state.stdout = String::new();
        }
        if !state.stderr.is_empty() {
            self.print_stderr(&state.stderr);
            state.stderr = String::new();
        }
    }

    fn print_stdout(&self, message: &str) {
        println!("[STDOUT   ] {}", message);
    }

    fn print_stderr(&self, message: &str) {
        println!("[STDERR   ] {}", message);
    }
}
