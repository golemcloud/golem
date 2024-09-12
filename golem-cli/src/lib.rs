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

use crate::service::version::{VersionCheckResult, VersionService};
use clap_verbosity_flag::Verbosity;
use colored::Colorize;
use lenient_bool::LenientBool;
use log::Level;
use tracing::warn;
use tracing_subscriber::FmtSubscriber;

pub mod clients;
pub mod cloud;
pub mod command;
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

pub fn init_tracing(verbosity: &Verbosity) {
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

            warn(format!("Warning: golem-cli version ({cli_version}) is older than the targeted Golem server version ({server_version})!"));
            warn("Download and install the latest version: https://github.com/golemcloud/golem-cloud-releases/releases");
            warn("(For more information see: https://learn.golem.cloud/docs/quickstart)");
        }
        Err(error) => {
            warn!("{}", error.0)
        }
    }
}
