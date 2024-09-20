use crate::components::{TestDependencies, ROOT_TOKEN};
use golem_cli::model::Format;
use golem_common::model::trim_date::TrimDateTime;
use libtest_mimic::Failed;
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
    fn run<T: DeserializeOwned, S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<T, Failed>;
    fn run_trimmed<T: DeserializeOwned + TrimDateTime, S: AsRef<OsStr> + Debug>(
        &self,
        args: &[S],
    ) -> Result<T, Failed>;
    fn run_string<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<String, Failed>;
    fn run_json<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Value, Failed>;
    fn run_unit<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<(), Failed>;
    fn run_stdout<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Child, Failed>;
}

#[derive(Debug, Clone)]
pub struct CliLive {
    pub config: CliConfig,
    golem_cli_path: PathBuf,
    format: Format,
    config_dir: PathBuf,
}

impl CliLive {
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
    ) -> Result<CliLive, Failed> {
        let config_dir = PathBuf::from(format!("../target/cli_conf/{conf_dir_name}"));
        let _ = fs::remove_dir_all(&config_dir);

        let golem_cli_path = PathBuf::from("../target/debug/golem-cloud-cli");

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
            let cloud_base_url = format!(
                "http://localhost:{}",
                deps.cloud_service().public_http_port()
            );
            let worker_base_url = format!(
                "http://localhost:{}",
                deps.worker_service().public_http_port()
            );

            cli.run_unit(&[
                "profile",
                "add",
                "--set-active",
                "--dev-component-url",
                &component_base_url,
                "--dev-cloud-url",
                &cloud_base_url,
                "--dev-worker-url",
                &worker_base_url,
                "cloud-default",
            ])?;

            Ok(cli)
        } else {
            Err(format!(
                "Expected to have precompiled Golem CLI at {}",
                golem_cli_path.to_str().unwrap_or("")
            )
            .into())
        }
    }

    fn run_inner<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<String, Failed> {
        println!(
            "Executing Golem CLI command: {} {args:?}",
            self.golem_cli_path.to_str().unwrap_or("")
        );

        let output = Command::new(&self.golem_cli_path)
            .env("NO_COLOR", "1")
            .env("GOLEM_CONFIG_DIR", self.config_dir.to_str().unwrap())
            .arg(self.config.arg('F', "format"))
            .arg(self.format.to_string())
            .arg(self.config.arg('T', "auth-token"))
            .arg(ROOT_TOKEN)
            .arg("-v")
            .args(args)
            .output()?;

        let stdout = String::from_utf8_lossy(output.stdout.as_slice()).to_string();
        let stderr = String::from_utf8_lossy(output.stderr.as_slice()).to_string();

        println!("CLI stdout: {stdout} for command {args:?}");
        println!("CLI stderr: {stderr} for command {args:?}");

        if !output.status.success() {
            return Err(format!(
                "golem cli failed with exit code: {:?}",
                output.status.code()
            )
            .into());
        }

        Ok(stdout)
    }
}

impl Cli for CliLive {
    fn run<'a, T: DeserializeOwned, S: AsRef<OsStr> + Debug>(
        &self,
        args: &[S],
    ) -> Result<T, Failed> {
        let stdout = self.run_inner(args)?;

        Ok(serde_json::from_str(&stdout)?)
    }

    fn run_trimmed<'a, T: DeserializeOwned + TrimDateTime, S: AsRef<OsStr> + Debug>(
        &self,
        args: &[S],
    ) -> Result<T, Failed> {
        let stdout = self.run_inner(args)?;
        Ok(serde_json::from_str::<T>(&stdout)?.trim_date_time_ms())
    }

    fn run_string<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<String, Failed> {
        self.run_inner(args)
    }

    fn run_json<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Value, Failed> {
        let stdout = self.run_inner(args)?;

        Ok(serde_json::from_str(&stdout)?)
    }

    fn run_unit<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<(), Failed> {
        let _ = self.run_inner(args)?;
        Ok(())
    }

    fn run_stdout<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Child, Failed> {
        println!(
            "Executing Golem CLI command: {} {args:?}",
            self.golem_cli_path.to_str().unwrap_or("")
        );

        let mut child = Command::new(&self.golem_cli_path)
            .env("GOLEM_CONFIG_DIR", self.config_dir.to_str().unwrap())
            .arg(self.config.arg('F', "format"))
            .arg(self.format.to_string())
            .arg(self.config.arg('T', "auth-token"))
            .arg(ROOT_TOKEN)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stderr = child
            .stderr
            .take()
            .ok_or::<Failed>("Can't get golem cli stderr".into())?;

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                eprintln!("[golem-cli-stderr]   {}", line.unwrap())
            }
        });

        Ok(child)
    }
}
