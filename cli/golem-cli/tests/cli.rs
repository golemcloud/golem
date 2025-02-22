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

use anyhow::anyhow;
use golem_cli::model::Format;
use golem_common::model::trim_date::TrimDateTime;
use golem_test_framework::config::TestDependencies;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct CliConfig {
    pub short_args: bool,
}

impl CliConfig {
    pub fn arg<S: Into<String>>(&self, short: char, long: S) -> String {
        if self.short_args {
            format!("-{short}")
        } else {
            format!("--{}", long.into())
        }
    }
}

pub trait Cli {
    fn run<T: DeserializeOwned, S: AsRef<OsStr> + Debug>(
        &self,
        args: &[S],
    ) -> Result<T, anyhow::Error>;
    fn run_trimmed<T: DeserializeOwned + TrimDateTime, S: AsRef<OsStr> + Debug>(
        &self,
        args: &[S],
    ) -> Result<T, anyhow::Error>;
    fn run_string<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<String, anyhow::Error>;
    fn run_json<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Value, anyhow::Error>;
    fn run_unit<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<(), anyhow::Error>;
    fn run_stdout<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Child, anyhow::Error>;
}

#[derive(Debug, Clone)]
pub struct CliLive {
    pub config: CliConfig,
    golem_cli_path: PathBuf,
    format: Format,
    config_dir: PathBuf,
}

impl CliLive {
    pub fn with_args(&self, short: bool) -> Self {
        if short {
            self.with_short_args()
        } else {
            self.with_long_args()
        }
    }

    pub fn with_short_args(&self) -> Self {
        CliLive {
            config: CliConfig { short_args: true },
            golem_cli_path: self.golem_cli_path.clone(),
            format: self.format,
            config_dir: self.config_dir.clone(),
        }
    }

    pub fn with_long_args(&self) -> Self {
        CliLive {
            config: CliConfig { short_args: false },
            golem_cli_path: self.golem_cli_path.clone(),
            format: self.format,
            config_dir: self.config_dir.clone(),
        }
    }

    pub fn with_format(&self, format: Format) -> Self {
        CliLive {
            format,
            ..self.clone()
        }
    }

    pub fn make(
        conf_dir_name: &str,
        deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
    ) -> Result<CliLive, anyhow::Error> {
        let config_dir = PathBuf::from(format!("../target/cli_conf/{conf_dir_name}"));
        let _ = fs::remove_dir_all(&config_dir);

        let golem_cli_path = PathBuf::from("../target/debug/golem-cli");

        println!(
            "CLI with component port {} and worker port {}",
            deps.component_service().public_http_port(),
            deps.worker_service().public_http_port()
        );

        if golem_cli_path.exists() {
            let cli = CliLive {
                config: CliConfig { short_args: false },
                golem_cli_path,
                format: Format::Json,
                config_dir,
            };

            let component_base_url = format!(
                "http://localhost:{}",
                deps.component_service().public_http_port()
            );
            let worker_base_url = format!(
                "http://localhost:{}",
                deps.worker_service().public_http_port()
            );

            cli.run_unit(&[
                "profile",
                "add",
                "--set-active",
                "--component-url",
                &component_base_url,
                "--worker-url",
                &worker_base_url,
                "default",
            ])?;

            Ok(cli)
        } else {
            Err(anyhow!(
                "Expected to have precompiled Golem CLI at {}",
                golem_cli_path.to_str().unwrap_or("")
            ))
        }
    }

    fn run_inner<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<String, anyhow::Error> {
        println!(
            "Executing Golem CLI command: {} {args:?}",
            self.golem_cli_path.to_str().unwrap_or("")
        );

        let output = Command::new(&self.golem_cli_path)
            .env("NO_COLOR", "1")
            .env("GOLEM_CONFIG_DIR", self.config_dir.to_str().unwrap())
            .env("GOLEM_CONNECT_TIMEOUT", "PT10S")
            .env("GOLEM_READ_TIMEOUT", "PT5M")
            .arg(self.config.arg('F', "format"))
            .arg(self.format.to_string())
            .arg("-v")
            .args(args)
            .output()?;

        let stdout = String::from_utf8_lossy(output.stdout.as_slice()).to_string();
        let stderr = String::from_utf8_lossy(output.stderr.as_slice()).to_string();

        println!("CLI stdout: {stdout} for command {args:?}");
        println!("CLI stderr: {stderr} for command {args:?}");

        if !output.status.success() {
            return Err(anyhow!(
                "golem cli failed with exit code: {:?}",
                output.status.code()
            ));
        }

        Ok(stdout)
    }
}

impl Cli for CliLive {
    fn run<'a, T: DeserializeOwned, S: AsRef<OsStr> + Debug>(
        &self,
        args: &[S],
    ) -> Result<T, anyhow::Error> {
        let stdout = self.run_inner(args)?;

        Ok(serde_json::from_str(&stdout)?)
    }

    fn run_trimmed<'a, T: DeserializeOwned + TrimDateTime, S: AsRef<OsStr> + Debug>(
        &self,
        args: &[S],
    ) -> Result<T, anyhow::Error> {
        let stdout = self.run_inner(args)?;
        Ok(serde_json::from_str::<T>(&stdout)?.trim_date_time_ms())
    }

    fn run_string<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<String, anyhow::Error> {
        self.run_inner(args)
    }

    fn run_json<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Value, anyhow::Error> {
        let stdout = self.run_inner(args)?;

        Ok(serde_json::from_str(&stdout)?)
    }

    fn run_unit<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<(), anyhow::Error> {
        let _ = self.run_inner(args)?;
        Ok(())
    }

    fn run_stdout<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Child, anyhow::Error> {
        println!(
            "Executing Golem CLI command: {} {args:?}",
            self.golem_cli_path.to_str().unwrap_or("")
        );

        let mut child = Command::new(&self.golem_cli_path)
            .env("GOLEM_CONFIG_DIR", self.config_dir.to_str().unwrap())
            .env("GOLEM_CONNECT_TIMEOUT", "PT10S")
            .env("GOLEM_READ_TIMEOUT", "PT5M")
            .arg(self.config.arg('F', "format"))
            .arg(self.format.to_string())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stderr = child
            .stderr
            .take()
            .ok_or(anyhow!("Can't get golem cli stderr"))?;

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                eprintln!("[golem-cli-stderr]   {}", line.unwrap())
            }
        });

        Ok(child)
    }
}
