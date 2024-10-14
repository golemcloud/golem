// Copyright 2024 Golem Cloud
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

use crate::config::ProfileName;
use crate::init::CliKind;
use crate::model::text::fmt::format_error;
use crate::model::{Format, GolemError, GolemResult, HasFormatConfig, HasVerbosity};
use crate::service::version::{VersionCheckResult, VersionService};
use clap_verbosity_flag::Verbosity;
use colored::Colorize;
use completion::PrintCompletion;
use golem_common::golem_version;
use lenient_bool::LenientBool;
use log::Level;
use std::future::Future;
use std::path::PathBuf;
use std::process::ExitCode;
use tracing::{info, warn};
use tracing_subscriber::FmtSubscriber;

pub mod clients;
pub mod cloud;
pub mod command;
pub mod completion;
pub mod config;
pub mod connect_output;
pub mod diagnose;
pub mod examples;
pub mod factory;
pub mod init;
pub mod model;
pub mod oss;
pub mod service;
pub mod stubgen;

#[cfg(test)]
test_r::enable!();

const VERSION: &str = golem_version!();

pub trait MainArgs {
    fn format(&self) -> Format;
    fn verbosity(&self) -> Verbosity;
    fn profile_name(&self) -> Option<&ProfileName>;
    fn cli_kind(&self) -> CliKind;
    fn args_kind(&self) -> &str;
}

pub struct InitMainArgs<Command>
where
    Command: HasFormatConfig + HasVerbosity + PrintCompletion,
{
    pub cli_kind: CliKind,
    pub config_dir: PathBuf,
    pub command: Command,
}

impl<Command> MainArgs for InitMainArgs<Command>
where
    Command: HasFormatConfig + HasVerbosity + PrintCompletion,
{
    fn format(&self) -> Format {
        self.command.format().unwrap_or_default()
    }

    fn verbosity(&self) -> Verbosity {
        self.command.verbosity()
    }

    fn profile_name(&self) -> Option<&ProfileName> {
        None
    }

    fn cli_kind(&self) -> CliKind {
        self.cli_kind
    }

    fn args_kind(&self) -> &str {
        "init"
    }
}

pub struct ConfiguredMainArgs<Profile, Command>
where
    Profile: HasFormatConfig,
    Command: HasFormatConfig + HasVerbosity + PrintCompletion,
{
    pub cli_kind: CliKind,
    pub config_dir: PathBuf,
    pub profile_name: ProfileName,
    pub profile: Profile,
    pub command: Command,
}

impl<Profile, Command> MainArgs for ConfiguredMainArgs<Profile, Command>
where
    Profile: HasFormatConfig,
    Command: HasFormatConfig + HasVerbosity + PrintCompletion,
{
    fn format(&self) -> Format {
        if let Some(format) = self.command.format() {
            return format;
        }
        if let Some(format) = self.profile.format() {
            return format;
        }
        Format::default()
    }

    fn verbosity(&self) -> Verbosity {
        self.command.verbosity()
    }

    fn profile_name(&self) -> Option<&ProfileName> {
        Some(&self.profile_name)
    }

    fn cli_kind(&self) -> CliKind {
        self.cli_kind
    }

    fn args_kind(&self) -> &str {
        "configured"
    }
}

pub fn run_main<F, A>(main: fn(A) -> F, args: A) -> ExitCode
where
    A: MainArgs,
    F: Future<Output = Result<GolemResult, GolemError>>,
{
    let format = args.format();
    init_tracing(args.verbosity());

    info!(
        args_king = args.args_kind(),
        cli_kind = format!("{:?}", args.cli_kind()),
        profile_name = format!("{:?}", args.profile_name()),
        format = format!("{:?}", format),
        "Starting Golem CLI",
    );

    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime for cli main")
        .block_on(main(args));

    match result {
        Ok(result) => {
            result.print(format);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", format_error(&error.0));
            ExitCode::FAILURE
        }
    }
}

pub fn parse_key_val(
    s: &str,
) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    Ok((s[..pos].parse()?, s[pos + 1..].parse()?))
}

pub fn parse_bool(s: &str) -> Result<bool, Box<dyn std::error::Error + Send + Sync + 'static>> {
    match s.parse::<LenientBool>() {
        Ok(b) => Ok(b.into()),
        Err(_) => Err(format!("invalid boolean: `{s}`"))?,
    }
}

pub fn init_tracing(verbosity: Verbosity) {
    if let Some(level) = verbosity.log_level() {
        let tracing_level = match level {
            Level::Error => tracing::Level::ERROR,
            Level::Warn => tracing::Level::WARN,
            Level::Info => tracing::Level::INFO,
            Level::Debug => tracing::Level::DEBUG,
            Level::Trace => tracing::Level::TRACE,
        };

        let subscriber = FmtSubscriber::builder()
            .with_max_level(tracing_level)
            .with_writer(std::io::stderr)
            .finish();

        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");
    }
}

pub async fn check_for_newer_server_version(
    version_service: &dyn VersionService,
    cli_version: &str,
) {
    match version_service.check(cli_version).await {
        Ok(VersionCheckResult::Ok) => { /* NOP */ }
        Ok(VersionCheckResult::NewerServerVersionAvailable {
            cli_version,
            server_version,
        }) => {
            fn warn<S: AsRef<str>>(line: S) {
                eprintln!("{}", line.as_ref().yellow());
            }

            warn(format!("\nWarning: golem-cli version ({cli_version}) is older than the targeted Golem server version ({server_version})!"));
            warn("Download and install the latest version: https://github.com/golemcloud/golem-cloud-releases/releases");
            warn("(For more information see: https://learn.golem.cloud/docs/quickstart)\n");
        }
        Err(error) => {
            warn!("{}", error.0)
        }
    }
}
