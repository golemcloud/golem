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

sequential_suite!(build_and_deploy_all);

tag_suite!(agents, group3);
sequential_suite!(agents);

inherit_test_dep!(Tracing);

use crate::{crate_path, workspace_path, Tracing};
use colored::Colorize;
use golem_cli::fs::{read_to_string, write_str};
use golem_client::api::HealthCheckClient;
use golem_client::Security;
use itertools::Itertools;
use lenient_bool::LenientBool;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::str::FromStr;
use std::time::Duration;
use tempfile::TempDir;
use test_r::{inherit_test_dep, sequential_suite, tag_suite};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::info;
use url::Url;

mod cmd {
    pub static NO_ARGS: &[&str] = &[];

    pub static AGENT: &str = "agent";
    pub static BUILD: &str = "build";
    pub static COMPLETION: &str = "completion";
    pub static COMPONENT: &str = "component";
    pub static DEPLOY: &str = "deploy";
    pub static GENERATE_BRIDGE: &str = "generate-bridge";
    pub static GET: &str = "get";
    pub static INVOKE: &str = "invoke";
    pub static LIST: &str = "list";
    pub static NEW: &str = "new";
    pub static PLUGIN: &str = "plugin";
    pub static REGISTER: &str = "register";
    pub static REPL: &str = "repl";
    pub static TEMPLATES: &str = "templates";
}

mod flag {
    pub static AGENT_TYPE_NAME: &str = "--agent-type-name";
    pub static DEV_MODE: &str = "--dev-mode";
    pub static FORCE_BUILD: &str = "--force-build";
    pub static FORMAT: &str = "--format";
    pub static LANGUAGE: &str = "--language";
    pub static SCRIPT: &str = "--script";
    pub static SHOW_SENSITIVE: &str = "--show-sensitive";
    pub static TEMPLATE_GROUP: &str = "--template-group";
    pub static YES: &str = "--yes";
}

mod pattern {
    pub static HELP_APPLICATION_COMPONENTS: &str = "Application components:";
    pub static HELP_APPLICATION_CUSTOM_COMMANDS: &str = "Application custom commands:";
    pub static HELP_USAGE: &str = "Usage:";
}

#[derive(Debug, Clone)]
enum CommandOutput {
    Stdout(String),
    Stderr(String),
}

pub struct Output {
    status: ExitStatus,
    output: Vec<CommandOutput>,
}

impl Output {
    pub async fn stream_and_collect(
        quiet: bool,
        prefix: &str,
        child: &mut Child,
    ) -> io::Result<Self> {
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
                    if !quiet {
                        println!("{prefix} {line}");
                    }
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
                    if !quiet {
                        println!("{prefix} {line}");
                    }
                    tx.send(CommandOutput::Stderr(line)).unwrap();
                }
            }
        });

        drop(tx);

        let mut output = vec![];
        while let Some(item) = rx.recv().await {
            output.push(item);
        }

        Ok(Self {
            status: child.wait().await?,
            output,
        })
    }

    fn stdout(&self) -> impl Iterator<Item = &str> {
        self.output.iter().filter_map(|output| match output {
            CommandOutput::Stdout(line) => Some(line.as_str()),
            CommandOutput::Stderr(_) => None,
        })
    }

    fn stderr(&self) -> impl Iterator<Item = &str> {
        self.output.iter().filter_map(|output| match output {
            CommandOutput::Stdout(_) => None,
            CommandOutput::Stderr(line) => Some(line.as_str()),
        })
    }

    #[must_use]
    fn success(&self) -> bool {
        self.status.success()
    }

    #[must_use]
    fn success_or_dump(&self) -> bool {
        let success = self.status.success();
        if !success {
            let std_out_prefix = "> golem-cli - stdout:".to_string().green().bold();
            let std_err_prefix = "> golem-cli - stderr:".to_string().red().bold();
            for output in &self.output {
                match output {
                    CommandOutput::Stdout(line) => {
                        println!("{} {}", std_out_prefix, line);
                    }
                    CommandOutput::Stderr(line) => {
                        println!("{} {}", std_err_prefix, line);
                    }
                }
            }
        }
        success
    }

    #[must_use]
    fn stdout_contains<S: AsRef<str>>(&self, text: S) -> bool {
        self.stdout()
            .map(strip_ansi_escapes::strip_str)
            .any(|line| line.contains(text.as_ref()))
    }

    fn stdout_contains_row_with_cells(&self, expected_cells: &[&str]) -> bool {
        self.stdout()
            .map(strip_ansi_escapes::strip_str)
            .any(|line| {
                let cells = line.split('|').map(str::trim).collect::<HashSet<_>>();
                expected_cells.iter().all(|cell| cells.contains(cell))
            })
    }

    #[must_use]
    fn stderr_contains<S: AsRef<str>>(&self, text: S) -> bool {
        self.stderr()
            .map(strip_ansi_escapes::strip_str)
            .any(|line| line.contains(text.as_ref()))
    }

    #[must_use]
    fn stdout_contains_ordered<S: AsRef<str>, I: IntoIterator<Item = S>>(
        &self,
        patterns: I,
    ) -> bool {
        contains_ordered(self.stdout(), patterns)
    }

    #[must_use]
    fn stderr_contains_ordered<S: AsRef<str>, I: IntoIterator<Item = S>>(
        &self,
        patterns: I,
    ) -> bool {
        contains_ordered(self.stderr(), patterns)
    }

    #[allow(dead_code)]
    #[must_use]
    fn stdout_count_lines_containing<S: AsRef<str>>(&self, text: S) -> usize {
        self.stdout()
            .filter(|line| line.contains(text.as_ref()))
            .count()
    }

    #[must_use]
    #[allow(dead_code)]
    fn stderr_count_lines_containing<S: AsRef<str>>(&self, text: S) -> usize {
        self.stderr()
            .filter(|line| line.contains(text.as_ref()))
            .count()
    }
}

#[derive(Debug)]
struct TestContext {
    quiet: bool,
    golem_path: PathBuf,
    golem_cli_path: PathBuf,
    _test_dir: TempDir,
    config_dir: TempDir,
    data_dir: TempDir,
    working_dir: PathBuf,
    server_process: Option<Child>,
    env: HashMap<String, String>,
    template_group: Option<String>,
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
        let quiet = std::env::var("QUIET")
            .ok()
            .and_then(|b| b.parse::<LenientBool>().ok())
            .unwrap_or_default()
            .0;

        let test_dir = TempDir::new().unwrap();
        let working_dir = test_dir.path().to_path_buf();

        let mut env = HashMap::new();

        env.insert(
            "GOLEM_ENABLE_WASMTIME_FS_CACHE".to_string(),
            "true".to_string(),
        );

        for key in [
            "GOLEM_RUST_PATH",
            "GOLEM_RUST_VERSION",
            "GOLEM_TS_PACKAGES_PATH",
            "GOLEM_TS_VERSION",
        ] {
            if let Ok(val) = std::env::var(key) {
                env.insert(key.to_string(), val);
            }
        }

        let ctx = Self {
            quiet,
            golem_path: {
                let path = workspace_path().join("target/debug/golem");
                if !path.exists() {
                    panic!("golem binary not found at {}", path.display());
                }
                path
            },
            golem_cli_path: {
                let path = workspace_path().join("target/debug/golem-cli");
                if !path.exists() {
                    panic!("golem-cli binary not found at {}", path.display());
                }
                path
            },
            _test_dir: test_dir,
            config_dir: TempDir::new().unwrap(),
            data_dir: TempDir::new().unwrap(),
            working_dir,
            server_process: None,
            env,
            template_group: None,
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

    #[allow(dead_code)]
    fn use_generic_template_group(&mut self) {
        self.use_template_group("generic")
    }

    fn use_template_group(&mut self, template_group: impl Into<String>) {
        self.template_group = Some(template_group.into());
    }

    #[allow(dead_code)]
    fn use_default_template_group(&mut self) {
        self.template_group = None;
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
            ];
            if let Some(template_group) = &self.template_group {
                all_args.push(flag::TEMPLATE_GROUP.to_string());
                all_args.push(template_group.to_string());
            }
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

        Output::stream_and_collect(self.quiet, "golem-cli", &mut child)
            .await
            .unwrap()
    }

    async fn start_server(&mut self) {
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

        let mut args = vec![
            "server",
            "run",
            "--config-dir",
            self.config_dir.path().to_str().unwrap(),
            "--data-dir",
            self.data_dir.path().to_str().unwrap(),
        ];

        if self.quiet {
            args.push("-q");
        }

        self.server_process = Some(
            Command::new(&self.golem_path)
                .args(&args)
                .current_dir(&self.working_dir)
                .spawn()
                .unwrap(),
        );

        {
            let start = Instant::now();
            let client = golem_client::api::HealthCheckClientLive {
                context: golem_client::Context {
                    client: reqwest::ClientBuilder::new()
                        .danger_accept_invalid_certs(true)
                        .build()
                        .expect("Failed to build reqwest client"),
                    base_url: Url::from_str("http://localhost:9881").unwrap(),
                    security_token: Security::Empty,
                },
            };
            let timeout = Duration::from_secs(10);
            let sleep_interval = Duration::from_millis(100);
            loop {
                match client.healthcheck().await {
                    Ok(_) => {
                        println!("> server healthcheck {}", "ok".green());
                        break;
                    }
                    Err(err) => {
                        if start.elapsed() > timeout {
                            println!(
                                "> server healthcheck failed: {}, stopping",
                                format!("{}", err).red()
                            );
                            panic!("Server is still not running, stopping");
                        } else {
                            println!(
                                "> server healthcheck failed: {}, retrying",
                                format!("{}", err).red()
                            );
                            tokio::time::sleep(sleep_interval).await;
                        }
                    }
                };
            }
        }
    }

    fn cd<P: AsRef<Path>>(&mut self, path: P) {
        self.working_dir = self.working_dir.join(path.as_ref());
    }

    fn cwd_path(&self) -> &Path {
        &self.working_dir
    }

    fn cwd_path_join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        self.working_dir.join(path)
    }

    fn test_data_path_join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        crate_path().join("test-data").join(path)
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
fn contains_ordered<LS, L, PS, P>(lines: L, patterns: P) -> bool
where
    LS: AsRef<str>,
    L: IntoIterator<Item = LS>,
    PS: AsRef<str>,
    P: IntoIterator<Item = PS>,
{
    let mut patterns = patterns.into_iter();
    let mut pattern = patterns.next();
    let mut pattern_str = pattern.as_ref().map(|s| s.as_ref());
    for line in lines {
        match pattern_str {
            Some(p) => {
                if line.as_ref().contains(p) {
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

pub fn replace_strings_in_file(
    path: impl AsRef<Path>,
    replace: &[(&str, &str)],
) -> anyhow::Result<()> {
    let path = path.as_ref();
    let mut content = read_to_string(path)?;
    for (from, to) in replace {
        content = content.replace(from, to);
    }
    write_str(path, content)
}

pub fn replace_string_in_file(path: impl AsRef<Path>, from: &str, to: &str) -> anyhow::Result<()> {
    replace_strings_in_file(path, &[(from, to)])
}
