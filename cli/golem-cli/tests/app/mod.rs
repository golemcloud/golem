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

mod agents;

#[allow(clippy::module_inception)]
mod app;

mod build_and_deploy_all;
mod plugins;

tag_suite!(app, group1);
sequential_suite!(app);

tag_suite!(plugins, group1);
sequential_suite!(plugins);

tag_suite!(build_and_deploy_all, group2);
sequential_suite!(build_and_deploy_all);

tag_suite!(agents, group3);
sequential_suite!(agents);

inherit_test_dep!(Tracing);

use crate::Tracing;
use assert2::assert;
use colored::Colorize;
use itertools::Itertools;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use tempfile::TempDir;
use test_r::{inherit_test_dep, sequential_suite, tag_suite};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::info;

mod cmd {
    pub static ADD_DEPENDENCY: &str = "add-dependency";
    pub static AGENT: &str = "agent";
    pub static APP: &str = "app";
    pub static BUILD: &str = "build";
    pub static COMPLETION: &str = "completion";
    pub static COMPONENT: &str = "component";
    pub static DEPLOY: &str = "deploy";
    pub static GET: &str = "get";
    pub static INVOKE: &str = "invoke";
    pub static NEW: &str = "new";
    pub static PLUGIN: &str = "plugin";
    pub static REGISTER: &str = "register";
    pub static REPL: &str = "repl";
    pub static TEMPLATES: &str = "templates";
}

mod flag {
    pub static DEV_MODE: &str = "--dev-mode";
    pub static FORCE_BUILD: &str = "--force-build";
    pub static REDEPLOY_ALL: &str = "--redeploy-all";
    pub static SCRIPT: &str = "--script";
    pub static FORMAT: &str = "--format";
    pub static SHOW_SENSITIVE: &str = "--show-sensitive";
    pub static YES: &str = "--yes";
}

mod pattern {
    pub static ERROR: &str = "error";
    pub static HELP_APPLICATION_COMPONENTS: &str = "Application components:";
    pub static HELP_APPLICATION_CUSTOM_COMMANDS: &str = "Application custom commands:";
    pub static HELP_COMMANDS: &str = "Commands:";
    pub static HELP_USAGE: &str = "Usage:";
}

enum CommandOutput {
    Stdout(String),
    Stderr(String),
}

pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
}

impl Output {
    pub async fn stream_and_collect(prefix: &str, child: &mut Child) -> io::Result<Self> {
        let stdout = child
            .stdout
            .take()
            .unwrap_or_else(|| panic!("Can't get {prefix} stdout"));

        let stderr = child
            .stderr
            .take()
            .unwrap_or_else(|| panic!("Can't get {prefix} stderr"));

        let (tx, mut rx) = mpsc::unbounded_channel();

        tokio::spawn({
            let prefix = format!("> {} - stdout:", prefix).green().bold();
            let tx = tx.clone();
            async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("{prefix} {line}");
                    tx.send(CommandOutput::Stdout(line)).unwrap();
                }
            }
        });

        tokio::spawn({
            let prefix = format!("> {} - stderr:", prefix).red().bold();
            let tx = tx.clone();
            async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("{prefix} {line}");
                    tx.send(CommandOutput::Stderr(line)).unwrap();
                }
            }
        });

        drop(tx);

        let mut stdout = vec![];
        let mut stderr = vec![];
        while let Some(item) = rx.recv().await {
            match item {
                CommandOutput::Stdout(line) => stdout.push(line),
                CommandOutput::Stderr(line) => stderr.push(line),
            }
        }

        Ok(Self {
            status: child.wait().await?,
            stdout,
            stderr,
        })
    }

    #[must_use]
    fn success(&self) -> bool {
        self.status.success()
    }

    #[must_use]
    fn stdout_contains<S: AsRef<str>>(&self, text: S) -> bool {
        self.stdout
            .iter()
            .map(strip_ansi_escapes::strip_str)
            .any(|line| line.contains(text.as_ref()))
    }

    fn stdout_contains_row_with_cells(&self, expected_cells: &[&str]) -> bool {
        self.stdout
            .iter()
            .map(strip_ansi_escapes::strip_str)
            .any(|line| {
                let cells = line.split('|').map(str::trim).collect::<HashSet<_>>();
                expected_cells.iter().all(|cell| cells.contains(cell))
            })
    }

    #[must_use]
    fn stderr_contains<S: AsRef<str>>(&self, text: S) -> bool {
        self.stderr
            .iter()
            .map(strip_ansi_escapes::strip_str)
            .any(|line| line.contains(text.as_ref()))
    }

    #[must_use]
    fn stdout_contains_ordered<S: AsRef<str>, I: IntoIterator<Item = S>>(
        &self,
        patterns: I,
    ) -> bool {
        contains_ordered(&self.stdout, patterns)
    }

    #[must_use]
    fn stderr_contains_ordered<S: AsRef<str>, I: IntoIterator<Item = S>>(
        &self,
        patterns: I,
    ) -> bool {
        contains_ordered(&self.stderr, patterns)
    }

    #[allow(dead_code)]
    #[must_use]
    fn stdout_count_lines_containing<S: AsRef<str>>(&self, text: S) -> usize {
        self.stdout
            .iter()
            .filter(|line| line.contains(text.as_ref()))
            .count()
    }

    #[must_use]
    fn stderr_count_lines_containing<S: AsRef<str>>(&self, text: S) -> usize {
        self.stderr
            .iter()
            .filter(|line| line.contains(text.as_ref()))
            .count()
    }
}

impl From<std::process::Output> for Output {
    fn from(output: std::process::Output) -> Self {
        fn to_lines(bytes: Vec<u8>) -> Vec<String> {
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|s| s.to_string())
                .collect()
        }

        Self {
            status: output.status,
            stdout: to_lines(output.stdout),
            stderr: to_lines(output.stderr),
        }
    }
}

#[derive(Debug)]
struct TestContext {
    golem_path: PathBuf,
    golem_cli_path: PathBuf,
    _test_dir: TempDir,
    config_dir: TempDir,
    data_dir: TempDir,
    working_dir: PathBuf,
    server_process: Option<Child>,
    env: HashMap<String, String>,
}

impl Drop for TestContext {
    fn drop(&mut self) {
        let server_process = self.server_process.take();
        tokio::spawn(async move {
            if let Some(mut server_process) = server_process {
                println!("{}", "> stopping golem server".bold());
                server_process.kill().await.unwrap();
            }
        });
    }
}

impl TestContext {
    fn new() -> Self {
        let test_dir = TempDir::new().unwrap();
        let working_dir = test_dir.path().to_path_buf();

        let ctx = Self {
            golem_path: PathBuf::from("../../target/debug/golem")
                .canonicalize()
                .unwrap_or_else(|_| {
                    panic!(
                        "golem binary not found in ../../target/debug/golem, with current dir: {:?}",
                        std::env::current_dir().unwrap()
                    );
                }),
            golem_cli_path: PathBuf::from("../../target/debug/golem-cli")
                .canonicalize()
                .unwrap_or_else(|_| {
                    panic!(
                        "golem binary not found in ../../target/debug/golem-cli, with current dir: {:?}",
                        std::env::current_dir().unwrap()
                    );
                }),
            _test_dir: test_dir,
            config_dir: TempDir::new().unwrap(),
            data_dir: TempDir::new().unwrap(),
            working_dir,
            server_process: None,
            env: HashMap::from_iter(vec![
                ("GOLEM_ENABLE_WASMTIME_FS_CACHE".to_string(), "true".to_string())
            ]),
        };

        info!(ctx = ?ctx ,"Created test context");

        ctx
    }

    #[allow(dead_code)]
    fn env(&self) -> &HashMap<String, String> {
        &self.env
    }

    fn env_mut(&mut self) -> &mut HashMap<String, String> {
        &mut self.env
    }

    fn add_env_var<K: Into<String>, V: Into<String>>(&mut self, key: K, value: V) {
        self.env_mut().insert(key.into(), value.into());
    }

    #[must_use]
    async fn cli<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = {
            let mut all_args = vec![
                "--config-dir".to_string(),
                self.config_dir.path().to_str().unwrap().to_string(),
                flag::DEV_MODE.to_string(),
            ];
            all_args.extend(
                args.into_iter()
                    .map(|a| a.as_ref().to_str().unwrap().to_string()),
            );
            all_args
        };
        let working_dir = &self.working_dir.canonicalize().unwrap();

        println!(
            "{} {}",
            "> working directory:".bold(),
            working_dir.display()
        );
        println!("{} {}", "> golem-cli".bold(), args.iter().join(" ").blue());

        let mut child = Command::new(&self.golem_cli_path)
            .args(args)
            .envs(&self.env)
            .current_dir(working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        Output::stream_and_collect("golem-cli", &mut child)
            .await
            .unwrap()
    }

    fn start_server(&mut self) {
        assert!(self.server_process.is_none(), "server is already running");

        println!("{}", "> starting golem server".bold());
        println!(
            "{} {}",
            "> server config directory:".bold(),
            self.config_dir.path().display()
        );
        println!(
            "{} {}",
            "> server data directory:".bold(),
            self.data_dir.path().display()
        );

        self.server_process = Some(
            Command::new(&self.golem_path)
                .args([
                    "server",
                    "run",
                    "--config-dir",
                    self.config_dir.path().to_str().unwrap(),
                    "--data-dir",
                    self.data_dir.path().to_str().unwrap(),
                ])
                .current_dir(&self.working_dir)
                .spawn()
                .unwrap(),
        )
    }

    fn cd<P: AsRef<Path>>(&mut self, path: P) {
        self.working_dir = self.working_dir.join(path.as_ref());
    }

    fn cwd_path_join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        self.working_dir.join(path)
    }
}

fn check_component_metadata(
    wasm: &Path,
    expected_package_name: String,
    expected_version: Option<String>,
) {
    let wasm = std::fs::read(wasm).unwrap();
    let payload = wasm_metadata::Payload::from_binary(&wasm).unwrap();
    let metadata = payload.metadata();

    assert_eq!(metadata.name, Some(expected_package_name));
    assert_eq!(
        metadata.version.as_ref().map(|v| v.to_string()),
        expected_version
    );
}

#[must_use]
fn contains_ordered<S: AsRef<str>, I: IntoIterator<Item = S>>(
    lines: &[String],
    patterns: I,
) -> bool {
    let mut patterns = patterns.into_iter();
    let mut pattern = patterns.next();
    let mut pattern_str = pattern.as_ref().map(|s| s.as_ref());
    for line in lines {
        match pattern_str {
            Some(p) => {
                if line.contains(p) {
                    pattern = patterns.next();
                    pattern_str = pattern.as_ref().map(|s| s.as_ref());
                }
            }
            None => {
                break;
            }
        }
    }
    let remaining_patterns = pattern_str
        .into_iter()
        .map(|s| s.to_string())
        .chain(patterns.map(|s| s.as_ref().to_string()))
        .collect::<Vec<_>>();
    if !remaining_patterns.is_empty() {
        println!("{}", "Missing patterns:".red().underline());
        for pattern in &remaining_patterns {
            println!("{pattern}");
        }
    }
    remaining_patterns.is_empty()
}
