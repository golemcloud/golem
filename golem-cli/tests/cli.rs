use golem_cli::model::Format;
use golem_test_framework::config::TestDependencies;
use libtest_mimic::Failed;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct CliConfig {
    short_args: bool,
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
    fn run<T: DeserializeOwned>(&self, args: &[&str]) -> Result<T, Failed>;
    fn run_string(&self, args: &[&str]) -> Result<String, Failed>;
    fn run_json(&self, args: &[&str]) -> Result<Value, Failed>;
    fn run_unit(&self, args: &[&str]) -> Result<(), Failed>;
    fn run_stdout(&self, args: &[&str]) -> Result<Child, Failed>;
}

#[derive(Debug, Clone)]
pub struct CliLive {
    pub config: CliConfig,
    golem_component_port: u16,
    golem_worker_port: u16,
    golem_cli_path: PathBuf,
    format: Format,
}

impl CliLive {
    pub fn with_short_args(&self) -> Self {
        CliLive {
            config: CliConfig { short_args: true },
            golem_component_port: self.golem_component_port,
            golem_worker_port: self.golem_worker_port,
            golem_cli_path: self.golem_cli_path.clone(),
            format: self.format,
        }
    }

    pub fn with_long_args(&self) -> Self {
        CliLive {
            config: CliConfig { short_args: false },
            golem_component_port: self.golem_component_port,
            golem_worker_port: self.golem_worker_port,
            golem_cli_path: self.golem_cli_path.clone(),
            format: self.format,
        }
    }

    pub fn with_format(&self, format: Format) -> Self {
        CliLive {
            format,
            ..self.clone()
        }
    }

    // TODO; Use NginxInfo
    pub fn make(
        deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
    ) -> Result<CliLive, Failed> {
        let golem_cli_path = PathBuf::from("../target/debug/golem-cli");

        println!(
            "CLI with component port {} and worker port {}",
            deps.component_service().public_http_port(),
            deps.worker_service().public_http_port()
        );

        if golem_cli_path.exists() {
            Ok(CliLive {
                config: CliConfig { short_args: false },
                golem_component_port: deps.component_service().public_http_port(),
                golem_worker_port: deps.worker_service().public_http_port(),
                golem_cli_path,
                format: Format::Json,
            })
        } else {
            Err(format!(
                "Expected to have precompiled Golem CLI at {}",
                golem_cli_path.to_str().unwrap_or("")
            )
            .into())
        }
    }

    fn component_base_url(&self) -> String {
        format!("http://localhost:{}", self.golem_component_port)
    }

    fn worker_base_url(&self) -> String {
        format!("http://localhost:{}", self.golem_worker_port)
    }

    fn run_inner<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<String, Failed> {
        println!(
            "Executing Golem CLI command: {} {args:?}",
            self.golem_cli_path.to_str().unwrap_or("")
        );

        let output = Command::new(&self.golem_cli_path)
            .env("GOLEM_COMPONENT_BASE_URL", self.component_base_url())
            .env("GOLEM_WORKER_BASE_URL", self.worker_base_url())
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
    fn run<'a, T: DeserializeOwned>(&self, args: &[&str]) -> Result<T, Failed> {
        let stdout = self.run_inner(args)?;

        Ok(serde_json::from_str(&stdout)?)
    }

    fn run_string(&self, args: &[&str]) -> Result<String, Failed> {
        self.run_inner(args)
    }

    fn run_json(&self, args: &[&str]) -> Result<Value, Failed> {
        let stdout = self.run_inner(args)?;

        Ok(serde_json::from_str(&stdout)?)
    }

    fn run_unit(&self, args: &[&str]) -> Result<(), Failed> {
        let _ = self.run_inner(args)?;
        Ok(())
    }

    fn run_stdout(&self, args: &[&str]) -> Result<Child, Failed> {
        println!(
            "Executing Golem CLI command: {} {args:?}",
            self.golem_cli_path.to_str().unwrap_or("")
        );

        let mut child = Command::new(&self.golem_cli_path)
            .env("GOLEM_COMPONENT_BASE_URL", self.component_base_url())
            .env("GOLEM_WORKER_BASE_URL", self.worker_base_url())
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
