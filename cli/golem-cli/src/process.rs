// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::error::PipedExitCode;
use crate::log::{logln, LogColorize, LogIndent};
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use colored::Colorize;
use gag::BufferRedirect;
use std::io::Read;
use std::io::Write;
use std::process::{ExitStatus, Stdio};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{enabled, Level};

pub trait ExitStatusExt {
    fn check_exit_status(&self) -> anyhow::Result<()>;
    fn pipe_exit_status(&self) -> anyhow::Result<()>;
}

impl ExitStatusExt for ExitStatus {
    fn check_exit_status(&self) -> anyhow::Result<()> {
        if self.success() {
            Ok(())
        } else {
            Err(anyhow!(format!(
                "Command failed with exit code: {}",
                self.code()
                    .map(|code| code.to_string().log_color_error_highlight().to_string())
                    .unwrap_or_else(|| "?".to_string())
            )))
        }
    }

    fn pipe_exit_status(&self) -> anyhow::Result<()> {
        if self.success() {
            Ok(())
        } else {
            Err(anyhow!(PipedExitCode(self.code().unwrap_or(1) as u8)))
        }
    }
}

#[async_trait]
pub trait CommandExt {
    async fn stream_and_wait_for_status(
        &mut self,
        command_name: &str,
    ) -> anyhow::Result<ExitStatus>;

    async fn stream_and_run(&mut self, command_name: &str) -> anyhow::Result<()> {
        self.stream_and_wait_for_status(command_name)
            .await?
            .check_exit_status()
    }

    fn stream_output(command_name: &str, child: &mut Child) -> anyhow::Result<()> {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stdout for {command_name}"))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stderr for {command_name}"))?;

        tokio::spawn({
            let prefix = format!("{} | ", command_name).green().bold();
            async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    logln(format!("{prefix} {line}"));
                }
            }
        });

        tokio::spawn({
            let prefix = format!("{} | ", command_name).yellow().bold();
            async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    logln(format!("{prefix} {line}"));
                }
            }
        });

        Ok(())
    }
}

#[async_trait]
impl CommandExt for Command {
    async fn stream_and_wait_for_status(
        &mut self,
        command_name: &str,
    ) -> anyhow::Result<ExitStatus> {
        let _indent = LogIndent::stash();

        let mut child = self
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn {command_name}"))?;

        Self::stream_output(command_name, &mut child)?;

        child
            .wait()
            .await
            .with_context(|| format!("Failed to execute {command_name}"))
    }
}

pub fn with_hidden_output_unless_error<F, R>(f: F) -> anyhow::Result<R>
where
    F: FnOnce() -> anyhow::Result<R>,
{
    let redirect = (!enabled!(Level::WARN))
        .then(|| BufferRedirect::stderr().ok())
        .flatten();

    let result = f();

    if result.is_err() {
        if let Some(mut redirect) = redirect {
            let mut output = Vec::new();
            let read_result = redirect.read_to_end(&mut output);
            drop(redirect);
            read_result.expect("Failed to read stderr from redirect");
            std::io::stderr()
                .write_all(output.as_slice())
                .expect("Failed to write captured stderr");
        }
    }

    result
}
