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

use assert2::{assert, check};
use colored::Colorize;
use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_templates::model::GuestLanguage;
use itertools::Itertools;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use strum::IntoEnumIterator;
use tempfile::TempDir;
use test_r::{test, test_dep};
use tracing::info;

test_r::enable!();

mod cmd {
    pub static APP: &str = "app";
    pub static COMPONENT: &str = "component";
    pub static BUILD: &str = "build";
    pub static NEW: &str = "new";
}

mod flag {
    pub static FORCE_BUILD: &str = "--force-build";
}

mod pattern {
    pub static ERROR: &str = "error";
    pub static HELP_APPLICATION_COMPONENTS: &str = "Application components:";
    pub static HELP_APPLICATION_CUSTOM_COMMANDS: &str = "Application custom commands:";
    pub static HELP_COMMANDS: &str = "Commands:";
    pub static HELP_USAGE: &str = "Usage:";
}

#[test]
fn app_help_in_empty_folder(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let outputs = ctx.cli([cmd::APP]);
    assert!(!outputs.success());
    check!(outputs.stderr_contains(pattern::HELP_USAGE));
    check!(outputs.stderr_contains(pattern::HELP_COMMANDS));
    check!(!outputs.stderr_contains(pattern::ERROR));
    check!(!outputs.stderr_contains(pattern::HELP_APPLICATION_COMPONENTS));
    check!(!outputs.stderr_contains(pattern::HELP_APPLICATION_CUSTOM_COMMANDS));
}

#[test]
fn app_new_with_many_components_and_then_help_in_app_folder(_tracing: &Tracing) {
    let app_name = "test-app-name";

    let mut ctx = TestContext::new();
    let outputs = ctx.cli([
        cmd::APP,
        cmd::NEW,
        app_name,
        "c",
        "go",
        "typescript",
        "rust",
    ]);
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "c", "app:c"]);
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "go", "app:go"]);
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "typescript", "app:typescript"]);
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"]);
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::APP]);
    assert!(!outputs.success());
    check!(outputs.stderr_contains(pattern::HELP_USAGE));
    check!(outputs.stderr_contains(pattern::HELP_COMMANDS));
    check!(!outputs.stderr_contains(pattern::ERROR));
    check!(outputs.stderr_contains(pattern::HELP_APPLICATION_COMPONENTS));
    check!(outputs.stderr_contains("app:c"));
    check!(outputs.stderr_contains("app:go"));
    check!(outputs.stderr_contains("app:rust"));
    check!(outputs.stderr_contains("app:typescript"));
    check!(outputs.stderr_contains(pattern::HELP_APPLICATION_CUSTOM_COMMANDS));
    check!(outputs.stderr_contains("cargo-clean"));
    check!(outputs.stderr_contains("npm-install"));
}

#[test]
fn app_build_with_rust_component(_tracing: &Tracing) {
    let app_name = "test-app-name";

    let mut ctx = TestContext::new();
    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]);
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "rust", "app:rust"]);
    assert!(outputs.success());

    // First build
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]);
    assert!(outputs.success());
    check!(outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(outputs.stderr_contains("Compiling app_rust v0.0.1"));

    // Rebuild - 1
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]);
    assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stderr_contains("Compiling app_rust v0.0.1"));

    // Rebuild - 2
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]);
    assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stderr_contains("Compiling app_rust v0.0.1"));

    // Rebuild - 3 - force, but cargo is smart to skip actual compile
    let outputs = ctx.cli([cmd::APP, cmd::BUILD, flag::FORCE_BUILD]);
    assert!(outputs.success());
    check!(outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(outputs.stderr_contains("Finished `dev` profile"));

    // Rebuild - 4
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]);
    assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stderr_contains("Compiling app_rust v0.0.1"));

    // Clean
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]);
    assert!(outputs.success());

    // Rebuild - 5
    let outputs = ctx.cli([cmd::APP, cmd::BUILD]);
    assert!(outputs.success());
    check!(!outputs.stdout_contains("Executing external command 'cargo component build'"));
    check!(!outputs.stderr_contains("Compiling app_rust v0.0.1"));
}

// TODO: re-enable once every language has templates
#[test]
fn app_new_language_hints(_tracing: &Tracing) {
    let ctx = TestContext::new();
    let outputs = ctx.cli([cmd::APP, cmd::NEW, "dummy-app-name"]);
    assert!(!outputs.success());
    check!(outputs.stderr_contains("Available languages:"));

    let languages_without_templates = GuestLanguage::iter()
        .filter(|language| !outputs.stderr_contains(format!("- {}", language)))
        .collect::<Vec<_>>();

    assert!(
        languages_without_templates.is_empty(),
        "{:?}",
        languages_without_templates
    );
}

pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
}

impl Output {
    fn success(&self) -> bool {
        self.status.success()
    }

    fn stdout_contains<S: AsRef<str>>(&self, text: S) -> bool {
        self.stdout.iter().any(|line| line.contains(text.as_ref()))
    }

    fn stderr_contains<S: AsRef<str>>(&self, text: S) -> bool {
        self.stderr.iter().any(|line| line.contains(text.as_ref()))
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
    _golem_path: PathBuf, // TODO:
    golem_cli_path: PathBuf,
    _test_dir: TempDir,
    _config_dir: TempDir, // TODO:
    working_dir: PathBuf,
}

impl TestContext {
    fn new() -> Self {
        let test_dir = TempDir::new().unwrap();
        let working_dir = test_dir.path().to_path_buf();

        let ctx = Self {
            _golem_path: PathBuf::from("../target/debug/golem")
                .canonicalize()
                .unwrap(),
            golem_cli_path: PathBuf::from("../target/debug/golem-cli")
                .canonicalize()
                .unwrap(),
            _test_dir: test_dir,
            _config_dir: TempDir::new().unwrap(),
            working_dir,
        };

        info!(ctx = ?ctx ,"Created test context");

        ctx
    }

    fn cli<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = args.into_iter().collect::<Vec<_>>();
        let working_dir = &self.working_dir.canonicalize().unwrap();

        println!(
            "{} {}",
            "> working directory:".bold(),
            working_dir.display()
        );
        println!(
            "{} {}",
            "> golem-cli".bold(),
            args.iter()
                .map(|s| s.as_ref().to_string_lossy())
                .join(" ")
                .blue()
        );

        let output: Output = Command::new(&self.golem_cli_path)
            .args(args)
            .current_dir(working_dir)
            .output()
            .unwrap()
            .into();

        let status_prefix = {
            let status_prefix = "> status:".bold();
            if output.success() {
                status_prefix.green()
            } else {
                status_prefix.red()
            }
        };
        println!("{} {}", status_prefix, output.status);
        let stdout_prefix = "> stdout:".green().bold();
        for line in &output.stdout {
            println!("{} {}", stdout_prefix, line);
        }
        let stderr_prefix = "> stderr:".red().bold();
        for line in &output.stderr {
            println!("{} {}", stderr_prefix, line);
        }

        output
    }

    fn cd<P: AsRef<Path>>(&mut self, path: P) {
        self.working_dir = self.working_dir.join(path.as_ref());
    }
}

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(&TracingConfig::test(
            "golem-cli-integration-tests",
        ));
        Self
    }
}

#[test_dep]
fn tracing() -> Tracing {
    Tracing::init()
}
