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

use crate::config::{NamedProfile, Profile};
use crate::init::CliKind;
use crate::model::text::fmt::format_error;
use crate::model::PrintRes;
use crate::service::version::{VersionCheckResult, VersionService};
use clap_verbosity_flag::Verbosity;
use colored::Colorize;
use command::profile::OssProfileAdd;
use command::{CliCommand, NoProfileCommandContext};
use config::{get_config_dir, Config};
use golem_common::golem_version;
use indoc::eprintdoc;
use lenient_bool::LenientBool;
use log::Level;
use oss::cli::{GolemOssCli, OssCommandContext};
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

#[cfg(test)]
test_r::enable!();

const VERSION: &str = golem_version!();

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

pub fn oss_main<ExtraCommands>() -> ExitCode
where
    ExtraCommands: CliCommand<OssCommandContext> + CliCommand<NoProfileCommandContext>,
{
    let config_dir = get_config_dir();

    let oss_profile = match Config::get_active_profile(CliKind::Oss, &config_dir) {
        Some(NamedProfile {
            name,
            profile: Profile::Golem(p),
        }) => Some((name, p)),
        Some(NamedProfile {
            name: _,
            profile: Profile::GolemCloud(_),
        }) => {
            eprintdoc!(
                    "Golem Cloud profile is not supported in this CLI version.
                    To use Golem Cloud please install golem-cloud-cli with feature 'universal' to replace golem-cli
                    cargo install --features universal golem-cloud-cli
                    And remove golem-cli:
                    cargo remove golem-cli
                    To create another default profile use `golem-cli init`.
                    To manage profiles use `golem-cli profile` command.
                    "
                );

            None
        }
        None => None,
    };

    let (command, parsed) =
        command::command_and_parsed::<GolemOssCli<OssProfileAdd, ExtraCommands>>();

    let format = parsed
        .format
        .or_else(|| oss_profile.as_ref().map(|(_, p)| p.config.default_format))
        .unwrap_or_default();

    init_tracing(parsed.verbosity);

    info!(
        profile = format!("{:?}", oss_profile.as_ref().map(|(n, _)| n)),
        format = format!("{:?}", format),
        "Starting Golem CLI",
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime for cli main");

    let cli_kind = CliKind::Oss;

    let result = if let Some((_, profile)) = oss_profile {
        runtime.block_on(oss::cli::run_with_profile(
            format, config_dir, profile, command, parsed, cli_kind,
        ))
    } else {
        runtime.block_on(oss::cli::run_without_profile(
            config_dir, command, parsed, cli_kind,
        ))
    };

    match result {
        Ok(result) => {
            result.println(format);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", format_error(&error.0));
            ExitCode::FAILURE
        }
    }
}
