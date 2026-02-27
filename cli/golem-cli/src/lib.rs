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

use anyhow::anyhow;
use clap_verbosity_flag::Verbosity;
use golem_common::tracing::directive;
use golem_common::tracing::directive::{debug, warn};
use lenient_bool::LenientBool;
use shadow_rs::shadow;
use std::future::Future;
use std::path::Path;
use std::process::ExitCode;
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, FmtSubscriber};

pub mod app;
pub mod auth;
pub mod bridge_gen;
pub mod client;
pub mod command;
pub mod command_handler;
pub mod composition;
pub mod config;
pub mod context;
pub mod diagnose;
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

#[macro_export]
macro_rules! app_manifest_version {
    () => {
        "1.5.0-dev.1"
    };
}
static GOLEM_RUST_VERSION: &str = "2.0.0-dev.2";
static GOLEM_TS_VERSION: &str = "0.1.0-dev.1";
static GOLEM_AI_VERSION: &str = "v0.5.0-dev.1";
static GOLEM_AI_SUFFIX: &str = "-dev.wasm";

static APP_MANIFEST_JSON_SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../schema.golem.cloud/app/golem/",
    app_manifest_version!(),
    "/golem.schema.json"
));

#[derive(Debug, Clone, Default)]
pub struct SdkOverrides {
    pub golem_rust_path: Option<String>,
    pub golem_rust_version: Option<String>,
    pub ts_packages_path: Option<String>,
    pub ts_version: Option<String>,
}

impl SdkOverrides {
    pub fn ts_package_dep(&self, package_name: &str) -> String {
        match &self.ts_packages_path {
            Some(ts_packages_path) => {
                format!("{}/{}", ts_packages_path, package_name)
            }
            None => self
                .ts_version
                .as_deref()
                .unwrap_or(GOLEM_TS_VERSION)
                .to_string(),
        }
    }

    pub fn golem_rust_dep(&self) -> String {
        match &self.golem_rust_path {
            Some(rust_path) => {
                format!(r#"path = "{}""#, rust_path)
            }
            _ => {
                format!(
                    r#"version = "{}""#,
                    self.golem_rust_version
                        .as_deref()
                        .unwrap_or(GOLEM_RUST_VERSION)
                )
            }
        }
    }

    pub fn golem_client_dep(&self) -> anyhow::Result<String> {
        if let Some(rust_path) = &self.golem_rust_path {
            return Ok(format!(
                r#"path = "{}/golem-client""#,
                Self::golem_repo_path_from_golem_rust_path(rust_path)?
            ));
        }

        todo!("No published version yet")
    }

    pub fn golem_repo_path_from_golem_rust_path(path: &str) -> anyhow::Result<String> {
        let suffix = Path::new("sdks/rust/golem-rust");
        let path = Path::new(path);
        fs::path_to_str(path)?
            .strip_suffix(fs::path_to_str(suffix)?)
            .ok_or_else(|| anyhow!("Invalid Golem Rust path: {}", path.display()))
            .map(|s| s.to_string())
    }
}

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
