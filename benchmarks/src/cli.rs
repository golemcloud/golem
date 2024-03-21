use crate::context::ContextInfo;
use anyhow::{anyhow, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

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
    fn run<'a, T: DeserializeOwned, S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<T>;
    fn run_json<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Value>;
    fn run_unit<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<()>;
    fn run_stdout<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Child>;
}

#[derive(Debug, Clone)]
pub struct CliLive {
    pub config: CliConfig,
    quiet: bool,
    schema: String,
    golem_template_host: String,
    golem_template_port: u16,
    golem_worker_host: String,
    golem_worker_port: u16,
    golem_cli_path: PathBuf,
}

impl CliLive {
    pub fn with_short_args(&self) -> Self {
        CliLive {
            config: CliConfig { short_args: true },
            quiet: self.quiet,
            schema: self.schema.clone(),
            golem_template_host: self.golem_template_host.clone(),
            golem_template_port: self.golem_template_port,
            golem_worker_host: self.golem_worker_host.clone(),
            golem_worker_port: self.golem_worker_port,
            golem_cli_path: self.golem_cli_path.clone(),
        }
    }

    pub fn with_long_args(&self) -> Self {
        CliLive {
            config: CliConfig { short_args: false },
            quiet: self.quiet,
            schema: self.schema.clone(),
            golem_template_host: self.golem_template_host.clone(),
            golem_template_port: self.golem_template_port,
            golem_worker_host: self.golem_worker_host.clone(),
            golem_worker_port: self.golem_worker_port,
            golem_cli_path: self.golem_cli_path.clone(),
        }
    }

    // TODO; Use NginxInfo
    pub fn make(context: &ContextInfo) -> Result<CliLive> {
        let golem_cli_path = PathBuf::from("../target/debug/golem-cli");

        if !context.env.quiet {
            println!(
                "CLI with template port {} and worker port {}",
                context.golem_template_service.local_http_port,
                context.golem_worker_service.local_http_port
            );
        }

        if golem_cli_path.exists() {
            Ok(CliLive {
                config: CliConfig { short_args: false },
                quiet: context.env.quiet,
                schema: context.env.schema.clone(),
                golem_template_host: context.golem_template_service.local_host.clone(),
                golem_template_port: context.golem_template_service.local_http_port,
                golem_worker_host: context.golem_worker_service.local_host.clone(),
                golem_worker_port: context.golem_worker_service.local_http_port,
                golem_cli_path,
            })
        } else {
            Err(anyhow!(
                "Expected to have precompiled Golem CLI at {}",
                golem_cli_path.to_str().unwrap_or("")
            ))
        }
    }

    fn template_base_url(&self) -> String {
        format!(
            "{}://{}:{}",
            self.schema, self.golem_template_host, self.golem_template_port
        )
    }

    fn worker_base_url(&self) -> String {
        format!(
            "{}://{}:{}",
            self.schema, self.golem_worker_host, self.golem_worker_port
        )
    }

    fn run_inner<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<String> {
        let quiet = self.quiet;

        if !quiet {
            println!(
                "Executing Golem CLI command: {} {args:?}",
                self.golem_cli_path.to_str().unwrap_or("")
            );
        }

        let output = Command::new(&self.golem_cli_path)
            .env("GOLEM_TEMPLATE_BASE_URL", self.template_base_url())
            .env("GOLEM_WORKER_BASE_URL", self.worker_base_url())
            .arg(self.config.arg('F', "format"))
            .arg("json")
            .arg("-v")
            .args(args)
            .output()?;

        let stdout = String::from_utf8_lossy(output.stdout.as_slice()).to_string();
        let stderr = String::from_utf8_lossy(output.stderr.as_slice()).to_string();

        if !quiet {
            println!("CLI stdout: {stdout} for command {args:?}");
            println!("CLI stderr: {stderr} for command {args:?}");
        }

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
    fn run<'a, T: DeserializeOwned, S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<T> {
        let stdout = self.run_inner(args)?;

        Ok(serde_json::from_str(&stdout)?)
    }

    fn run_json<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Value> {
        let stdout = self.run_inner(args)?;

        Ok(serde_json::from_str(&stdout)?)
    }

    fn run_unit<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<()> {
        let _ = self.run_inner(args)?;
        Ok(())
    }

    fn run_stdout<S: AsRef<OsStr> + Debug>(&self, args: &[S]) -> Result<Child> {
        println!(
            "Executing Golem CLI command: {} {args:?}",
            self.golem_cli_path.to_str().unwrap_or("")
        );

        let mut child = Command::new(&self.golem_cli_path)
            .env("GOLEM_TEMPLATE_BASE_URL", self.template_base_url())
            .env("GOLEM_WORKER_BASE_URL", self.worker_base_url())
            .arg(self.config.arg('F', "format"))
            .arg("json")
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
                eprintln!("[golem-cloud-cli-stderr]   {}", line.unwrap())
            }
        });

        Ok(child)
    }
}
