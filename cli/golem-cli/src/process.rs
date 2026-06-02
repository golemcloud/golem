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

use crate::error::PipedExitCode;
use crate::log::{LogColorize, LogIndent, logln};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use colored::Colorize;
use colored::control::SHOULD_COLORIZE;
use std::collections::HashMap;
#[cfg(windows)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{LazyLock, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use which::which as wrapped_which;
#[cfg(windows)]
use which::which_in as wrapped_which_in;

static PROGRAM_LOOKUP_CACHE: LazyLock<Mutex<HashMap<String, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn which(program: &str) -> anyhow::Result<PathBuf> {
    let program_path = Path::new(program);

    if is_explicit_program_path(program_path) {
        return Ok(program_path.to_path_buf());
    }

    if let Some(cached) = PROGRAM_LOOKUP_CACHE
        .lock()
        .expect("program lookup cache lock poisoned")
        .get(program)
        .cloned()
    {
        return cached.ok_or_else(|| anyhow!("Program '{}' not found on PATH", program));
    }

    let resolved = wrapped_which(program_path).ok();

    PROGRAM_LOOKUP_CACHE
        .lock()
        .expect("program lookup cache lock poisoned")
        .insert(program.to_string(), resolved.clone());

    resolved.ok_or_else(|| anyhow!("Program '{}' not found on PATH", program))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCommand {
    pub program: PathBuf,
    pub prefix_args: Vec<OsString>,
}

impl ResolvedCommand {
    fn direct(program: PathBuf) -> Self {
        Self {
            program,
            prefix_args: Vec::new(),
        }
    }
}

pub fn resolve_command_for_execution(
    program: &str,
    current_dir: Option<&Path>,
) -> anyhow::Result<ResolvedCommand> {
    let program_path = Path::new(program);

    #[cfg(windows)]
    {
        let resolved = if is_explicit_program_path(program_path) {
            resolve_explicit_program_path_on_windows(program_path, current_dir)?
        } else {
            which(program)?
        };

        return wrap_powershell_script_if_needed_on_windows(resolved);
    }

    #[cfg(not(windows))]
    {
        let _ = current_dir;
        if is_explicit_program_path(program_path) {
            Ok(ResolvedCommand::direct(program_path.to_path_buf()))
        } else {
            which(program).map(ResolvedCommand::direct)
        }
    }
}

#[cfg(windows)]
fn resolve_explicit_program_path_on_windows(
    program_path: &Path,
    current_dir: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let base_dir = match current_dir {
        Some(current_dir) => current_dir.to_path_buf(),
        None => std::env::current_dir().map_err(anyhow::Error::from)?,
    };

    let fallback_path = if program_path.is_absolute() {
        program_path.to_path_buf()
    } else {
        crate::fs::normalize_path_lexically(&base_dir.join(program_path))
    };

    if let Ok(resolved) = wrapped_which_in(program_path, None::<&OsStr>, &base_dir) {
        return Ok(resolved);
    }

    // MoonBit bin-deps install Windows launchers as .ps1 scripts without the
    // .cmd/.bat variants that tools like npm provide. .ps1 is not part of the
    // default PATHEXT, so add this fallback only for explicit paths while keeping
    // bare command lookup tied to the user's PATHEXT configuration.
    if program_path.extension().is_none() {
        let ps1_candidate = append_extension(&fallback_path, ".ps1");
        if ps1_candidate.exists() {
            return Ok(ps1_candidate);
        }
    }

    Ok(fallback_path)
}

#[cfg(windows)]
fn append_extension(path: &Path, extension: &str) -> PathBuf {
    let mut path = path.as_os_str().to_os_string();
    path.push(extension);
    PathBuf::from(path)
}

#[cfg(windows)]
fn wrap_powershell_script_if_needed_on_windows(
    program: PathBuf,
) -> anyhow::Result<ResolvedCommand> {
    if program
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"))
    {
        // .ps1 files are PowerShell scripts, not Windows process images. Node/npm
        // provide .cmd launchers that already work with Command::new; MoonBit only
        // provides the .ps1 launcher, so run it through PowerShell explicitly.
        Ok(ResolvedCommand {
            program: which("powershell.exe")?,
            prefix_args: vec![
                OsString::from("-NoProfile"),
                OsString::from("-ExecutionPolicy"),
                OsString::from("Bypass"),
                OsString::from("-File"),
                program.into_os_string(),
            ],
        })
    } else {
        Ok(ResolvedCommand::direct(program))
    }
}

pub fn normalized_program_name(program: &str) -> String {
    let path = Path::new(program);
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_explicit_program_path(path: &Path) -> bool {
    path.is_absolute() || path.components().count() > 1
}

#[cfg(test)]
fn clear_program_lookup_cache() {
    PROGRAM_LOOKUP_CACHE
        .lock()
        .expect("program lookup cache lock poisoned")
        .clear();
}

#[cfg(test)]
fn program_lookup_cache_len() -> usize {
    PROGRAM_LOOKUP_CACHE
        .lock()
        .expect("program lookup cache lock poisoned")
        .len()
}

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

#[derive(Clone, Copy)]
enum StreamKind {
    Stdout,
    Stderr,
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

    fn stream_output_prefix(command_name: &str, is_stderr: bool) -> String {
        if !SHOULD_COLORIZE.should_colorize() {
            return format!("{command_name} |");
        }

        let (badge, separator) = if is_stderr {
            (
                format!(" {} ", command_name)
                    .on_bright_black()
                    .yellow()
                    .bold()
                    .to_string(),
                "│".yellow().bold().to_string(),
            )
        } else {
            (
                format!(" {} ", command_name)
                    .on_bright_black()
                    .green()
                    .bold()
                    .to_string(),
                "│".green().bold().to_string(),
            )
        };

        format!("{badge}{separator}")
    }

    fn stream_output(command_name: &str, child: &mut Child) -> anyhow::Result<Vec<JoinHandle<()>>> {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stdout for {command_name}"))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stderr for {command_name}"))?;

        let (tx, mut rx) = mpsc::unbounded_channel::<(StreamKind, String)>();

        let stdout_task = tokio::spawn({
            let tx = tx.clone();
            async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send((StreamKind::Stdout, line));
                }
            }
        });

        let stderr_task = tokio::spawn({
            let tx = tx.clone();
            async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send((StreamKind::Stderr, line));
                }
            }
        });
        drop(tx);

        let printer_task = tokio::spawn({
            let command_name = command_name.to_string();
            async move {
                let color = SHOULD_COLORIZE.should_colorize();
                let (top_frame, bottom_frame) = if color {
                    let width = command_name.chars().count() + 2;
                    (
                        Some("▁".repeat(width).bright_black().to_string()),
                        Some("▔".repeat(width).bright_black().to_string()),
                    )
                } else {
                    (None, None)
                };

                let stdout_prefix = Self::stream_output_prefix(&command_name, false);
                let stderr_prefix = Self::stream_output_prefix(&command_name, true);

                let mut printed_top = false;
                let mut had_any_output = false;

                while let Some((stream_kind, line)) = rx.recv().await {
                    if color && !printed_top {
                        if let Some(top_frame) = &top_frame {
                            logln(top_frame);
                        }
                        printed_top = true;
                    }

                    let prefix = match stream_kind {
                        StreamKind::Stdout => &stdout_prefix,
                        StreamKind::Stderr => &stderr_prefix,
                    };

                    logln(format!("{prefix} {line}"));
                    had_any_output = true;
                }

                if color
                    && had_any_output
                    && let Some(bottom_frame) = &bottom_frame
                {
                    logln(bottom_frame);
                }
            }
        });

        Ok(vec![stdout_task, stderr_task, printer_task])
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

        let stream_tasks = Self::stream_output(command_name, &mut child)?;

        let status = child
            .wait()
            .await
            .with_context(|| format!("Failed to execute {command_name}"))?;

        for task in stream_tasks {
            let _ = task.await;
        }

        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn which_uses_cache_for_identical_lookup() {
        clear_program_lookup_cache();

        #[cfg(windows)]
        let requested_program_name = "cargo.exe";

        #[cfg(not(windows))]
        let requested_program_name = "cargo";

        let first = which(requested_program_name).unwrap();
        let cached = which(requested_program_name).unwrap();

        assert_eq!(first, cached);
        assert_eq!(program_lookup_cache_len(), 1);
    }

    #[test]
    fn normalized_program_name_drops_extension_and_normalizes_case() {
        assert_eq!(normalized_program_name("npm"), "npm");
        assert_eq!(normalized_program_name("NPM.CMD"), "npm");
        assert_eq!(normalized_program_name("C:/Tools/Npx.ExE"), "npx");
    }
}
