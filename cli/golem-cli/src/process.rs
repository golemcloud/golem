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
use crate::log::{logln, LogColorize, LogIndent};
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use colored::Colorize;
use gag::BufferRedirect;
use std::env;
use std::ffi::OsString;
use std::collections::HashMap;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{LazyLock, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
struct ProgramLookupCacheKey {
    program: String,
    path_var: OsString,
    pathext_var: Option<OsString>,
}

static PROGRAM_LOOKUP_CACHE: LazyLock<Mutex<HashMap<ProgramLookupCacheKey, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// TODO: agent: do we have to do this for every platform? maybe we shoul just only apply all this lookup for windows?
pub fn resolve_program_for_spawn(program: &str) -> anyhow::Result<PathBuf> {
    let Some(path_var) = env::var_os("PATH") else {
        return Err(anyhow!(
            "Program '{}' was not found: PATH is not set",
            program
        ));
    };
    let pathext_var = env::var_os("PATHEXT");

    resolve_program_for_spawn_from_env(program, path_var, pathext_var)
}

fn resolve_program_for_spawn_from_env(
    program: &str,
    path_var: OsString,
    pathext_var: Option<OsString>,
) -> anyhow::Result<PathBuf> {
    let program_path = Path::new(program);

    if is_explicit_program_path(program_path) {
        return Ok(program_path.to_path_buf());
    }

    let cache_key = ProgramLookupCacheKey {
        program: program.to_string(),
        path_var: path_var.clone(),
        pathext_var: pathext_var.clone(),
    };

    if let Some(cached) = PROGRAM_LOOKUP_CACHE
        .lock()
        .expect("program lookup cache lock poisoned")
        .get(&cache_key)
        .cloned()
    {
        return cached.ok_or_else(|| anyhow!("Program '{}' not found on PATH", program));
    }

    let resolved = resolve_program_on_path(program_path, &path_var, pathext_var.as_ref());

    PROGRAM_LOOKUP_CACHE
        .lock()
        .expect("program lookup cache lock poisoned")
        .insert(cache_key, resolved.clone());

    resolved.ok_or_else(|| anyhow!("Program '{}' not found on PATH", program))
}

fn resolve_program_on_path(
    program_path: &Path,
    path_var: &OsString,
    pathext_var: Option<&OsString>,
) -> Option<PathBuf> {
    for dir in env::split_paths(path_var) {
        for candidate in candidate_program_paths(&dir, program_path, pathext_var) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
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

#[cfg(not(windows))]
fn candidate_program_paths(
    dir: &Path,
    program: &Path,
    _pathext_var: Option<&OsString>,
) -> Vec<PathBuf> {
    vec![dir.join(program)]
}

#[cfg(windows)]
fn candidate_program_paths(
    dir: &Path,
    program: &Path,
    pathext_var: Option<&OsString>,
) -> Vec<PathBuf> {
    if program.extension().is_some() {
        return vec![dir.join(program)];
    }

    windows_pathexts(pathext_var)
        .into_iter()
        .map(|ext| {
            let mut file_name = OsString::from(program.as_os_str());
            file_name.push(ext);
            dir.join(file_name)
        })
        .collect()
}

#[cfg(windows)]
fn windows_pathexts(pathext_var: Option<&OsString>) -> Vec<String> {
    const DEFAULT_EXTS: [&str; 4] = [".exe", ".cmd", ".bat", ".com"];

    let mut result = Vec::new();

    if let Some(pathext) = pathext_var {
        for ext in pathext
            .to_string_lossy()
            .split(';')
            .map(str::trim)
            .filter(|ext| !ext.is_empty())
        {
            let normalized = if ext.starts_with('.') {
                ext.to_ascii_lowercase()
            } else {
                format!(".{}", ext.to_ascii_lowercase())
            };

            if !result.iter().any(|known| known == &normalized) {
                result.push(normalized);
            }
        }
    }

    for ext in DEFAULT_EXTS {
        if !result.iter().any(|known| known == ext) {
            result.push(ext.to_string());
        }
    }

    result
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

pub enum HiddenOutput {
    Stdout,
    Stderr,
    All,
    None,
}

impl HiddenOutput {
    pub fn hide_stderr_if(cond: bool) -> Self {
        if cond {
            Self::Stderr
        } else {
            Self::None
        }
    }

    fn should_hide_stdout(&self) -> bool {
        matches!(self, Self::Stdout | Self::All)
    }
    fn should_hide_stderr(&self) -> bool {
        matches!(self, Self::Stderr | Self::All)
    }
}

pub fn with_hidden_output_unless_error<F, R>(hidden_output: HiddenOutput, f: F) -> anyhow::Result<R>
where
    F: FnOnce() -> anyhow::Result<R>,
{
    let stdout_redirect = (hidden_output.should_hide_stdout())
        .then(|| BufferRedirect::stdout().ok())
        .flatten();

    let stderr_redirect = (hidden_output.should_hide_stderr())
        .then(|| BufferRedirect::stderr().ok())
        .flatten();

    let result = f();

    if result.is_err() {
        if let Some(mut redirect) = stdout_redirect {
            let mut output = Vec::new();
            let read_result = redirect.read_to_end(&mut output);
            drop(redirect);
            read_result.expect("Failed to read stdout from redirect");
            std::io::stdout()
                .write_all(output.as_slice())
                .expect("Failed to write captured stdout");
        }
        if let Some(mut redirect) = stderr_redirect {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use test_r::test;

    #[test]
    fn resolve_program_for_spawn_uses_cache_for_identical_lookup() {
        clear_program_lookup_cache();

        let bin_dir = tempdir().unwrap();
        let program_name = "demo-tool";

        #[cfg(windows)]
        let (program_file_name, pathext_var) = ("demo-tool.cmd", Some(OsString::from(".CMD")));

        #[cfg(not(windows))]
        let (program_file_name, pathext_var) = ("demo-tool", None);

        let program_path = bin_dir.path().join(program_file_name);
        std::fs::write(&program_path, "").unwrap();

        let path_var = bin_dir.path().as_os_str().to_os_string();

        let first =
            resolve_program_for_spawn_from_env(program_name, path_var.clone(), pathext_var.clone())
                .unwrap();
        let cached =
            resolve_program_for_spawn_from_env(program_name, path_var.clone(), pathext_var).unwrap();

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
