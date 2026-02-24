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

use clap_verbosity_flag::Verbosity;
use golem_common::tracing::directive;
use golem_common::tracing::directive::{debug, warn};
use lenient_bool::LenientBool;
use shadow_rs::shadow;
use std::future::Future;
use std::process::ExitCode;
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, FmtSubscriber};

pub mod app;
pub mod app_template;
pub mod auth;
pub mod bridge_gen;
pub mod client;
pub mod command;
pub mod command_handler;
pub mod composition;
pub mod config;
pub mod context;
pub mod diagnose;
pub mod edit;
pub mod error;
pub mod evcxr_repl;
pub mod fs;
pub mod fuzzy;
pub mod log;
pub mod model;
pub mod process;
pub mod validation;

#[cfg(test)]
test_r::enable!();

shadow!(build);

pub fn command_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or("golem-cli".to_string())
}

pub fn binary_path_to_string() -> anyhow::Result<String> {
    Ok(fs::path_to_str(&std::env::current_exe()?)?.to_string())
}

pub fn version() -> &'static str {
    if build::PKG_VERSION != "0.0.0" {
        build::PKG_VERSION
    } else {
        build::GIT_DESCRIBE_TAGS
    }
}

pub fn init_tracing(verbosity: Verbosity, pretty_mode: bool) {
    if let Some(level) = verbosity.tracing_level() {
        let subscriber = FmtSubscriber::builder();

        let mut filter = EnvFilter::builder().parse_lossy(level.as_str());
        for directive in directive::default_deps() {
            filter = filter.add_directive(directive);
        }
        filter = filter.add_directive(warn("opentelemetry_sdk"));
        filter = filter.add_directive(warn("opentelemetry"));
        filter = filter.add_directive(warn("poem"));
        filter = filter.add_directive(
            // Special case: only show sqlx debug logs on TRACE level
            if level == tracing::Level::TRACE {
                debug("sqlx")
            } else {
                warn("sqlx")
            },
        );

        if pretty_mode {
            let subscriber = subscriber
                .pretty()
                .with_max_level(level)
                .with_writer(std::io::stderr)
                .with_env_filter(filter)
                .finish();

            tracing::subscriber::set_global_default(subscriber)
                .expect("setting default subscriber failed");
        } else {
            let subscriber = subscriber
                .with_max_level(level)
                .with_writer(std::io::stderr)
                .with_env_filter(filter)
                .finish();

            tracing::subscriber::set_global_default(subscriber)
                .expect("setting default subscriber failed");
        };

        LogTracer::init().expect("failed to initialize log tracer");
    }
}

pub fn main_wrapper<F>(golem_main: impl FnOnce() -> F) -> ExitCode
where
    F: Future<Output = ExitCode>,
{
    if is_golem_evcxr_repl_set() {
        evcxr_repl::main()
    } else {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime")
            .block_on(golem_main())
    }
}

pub const GOLEM_EVCXR_REPL: &str = "GOLEM_EVCXR_REPL";

fn is_golem_evcxr_repl_set() -> bool {
    std::env::var(GOLEM_EVCXR_REPL)
        .ok()
        .and_then(|s| s.parse::<LenientBool>().ok())
        .map(|b| b.into())
        .unwrap_or(false)
}
