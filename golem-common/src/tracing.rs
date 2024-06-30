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

use std::fs::File;
use std::io::stdout;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::info;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;

pub enum Output {
    Stdout,
    File,
    TracingConsole,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OutputConfig {
    pub enabled: bool,
    pub json: bool,
    pub json_flatten: bool,
    pub ansi: bool,
    pub span_active: bool,
    pub span_full: bool,
}

impl OutputConfig {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn text() -> Self {
        Self {
            enabled: true,
            json: false,
            json_flatten: false,
            ansi: false,
            span_active: false,
            span_full: false,
        }
    }

    pub fn text_ansi() -> Self {
        Self {
            enabled: true,
            json: false,
            json_flatten: false,
            ansi: true,
            span_active: false,
            span_full: false,
        }
    }

    pub fn json() -> Self {
        Self {
            enabled: true,
            json: true,
            json_flatten: false,
            ansi: false,
            span_active: false,
            span_full: false,
        }
    }

    pub fn json_flattened() -> Self {
        Self {
            enabled: true,
            json: true,
            json_flatten: true,
            ansi: false,
            span_active: false,
            span_full: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub stdout: OutputConfig,
    pub file: OutputConfig,
    pub file_dir: Option<String>,
    pub file_name: Option<String>,
    pub console: bool,
}

impl Config {
    pub fn local_dev(service_name: &str) -> Self {
        Self {
            stdout: OutputConfig::text_ansi(),
            file: OutputConfig {
                enabled: false,
                ..OutputConfig::json_flattened()
            },
            file_dir: None,
            file_name: Some(format!("{}.log", service_name)),
            console: false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            stdout: OutputConfig::json_flattened(),
            file: OutputConfig {
                enabled: false,
                ..OutputConfig::json_flattened()
            },
            file_dir: None,
            file_name: None,
            console: false,
        }
    }
}

pub mod directive {
    use tracing_subscriber::filter::Directive;

    pub mod default {
        use tracing_subscriber::filter::Directive;

        pub fn debug() -> Directive {
            "debug".parse().unwrap()
        }

        pub fn info() -> Directive {
            "info".parse().unwrap()
        }

        pub fn warn() -> Directive {
            "warn".parse().unwrap()
        }

        pub fn error() -> Directive {
            "error".parse().unwrap()
        }
    }

    pub fn debug(target: &str) -> Directive {
        format!("{}=debug", target).parse().unwrap()
    }

    pub fn info(target: &str) -> Directive {
        format!("{}=info", target).parse().unwrap()
    }

    pub fn warn(target: &str) -> Directive {
        format!("{}=warn", target).parse().unwrap()
    }

    pub fn error(target: &str) -> Directive {
        format!("{}=error", target).parse().unwrap()
    }

    pub fn default_deps() -> Vec<Directive> {
        vec![
            warn("cranelift_codegen"),
            warn("wasmtime_cranelift"),
            warn("wasmtime_jit"),
            warn("h2"),
            warn("hyper"),
            warn("tower"),
            warn("fred"),
        ]
    }
}

pub mod filter {
    use tracing_subscriber::layer::Filter;
    use tracing_subscriber::Registry;

    pub type Boxed = Box<dyn Filter<Registry> + 'static + Send + Sync>;

    pub mod boxed {
        use tracing_subscriber::filter::Directive;
        use tracing_subscriber::EnvFilter;

        use crate::tracing::directive;
        use crate::tracing::filter::Boxed;

        pub fn default_env() -> Boxed {
            Box::new(EnvFilter::from_default_env())
        }

        pub fn env_with_directives(
            default_directive: Directive,
            directives: Vec<Directive>,
        ) -> Boxed {
            let mut builder = EnvFilter::builder()
                .with_default_directive(default_directive)
                .from_env_lossy();

            for directive in directives {
                builder = builder.add_directive(directive);
            }

            Box::new(builder)
        }

        pub fn debug_env_with_directives(directives: Vec<Directive>) -> Boxed {
            env_with_directives(directive::default::debug(), directives)
        }

        pub fn default_debug_env() -> Boxed {
            debug_env_with_directives(directive::default_deps())
        }

        pub fn info_env_with_directives(directives: Vec<Directive>) -> Boxed {
            env_with_directives(directive::default::info(), directives)
        }

        pub fn default_info_env() -> Boxed {
            env_with_directives(directive::default::info(), directive::default_deps())
        }
    }

    pub mod for_all_outputs {
        use tracing_subscriber::filter::Directive;

        use crate::tracing::filter::{boxed, Boxed};
        use crate::tracing::Output;

        pub const DEFAULT_ENV: fn(Output) -> Boxed = |_output| boxed::default_env();

        pub fn debug_env_with_directives(directives: Vec<Directive>) -> impl Fn(Output) -> Boxed {
            move |_output| boxed::debug_env_with_directives(directives.clone())
        }

        pub fn default_debug_env() -> impl Fn(Output) -> Boxed {
            move |_output| boxed::default_debug_env()
        }

        pub fn info_env_with_directives(directives: Vec<Directive>) -> impl Fn(Output) -> Boxed {
            move |_output| boxed::info_env_with_directives(directives.clone())
        }

        pub fn default_info_env() -> impl Fn(Output) -> Boxed {
            move |_output| boxed::default_info_env()
        }
    }
}

pub fn init<F>(config: &Config, make_filter: F)
where
    F: Fn(Output) -> filter::Boxed,
{
    let mut layers = Vec::new();

    if config.stdout.enabled {
        layers.push(make_layer(
            &config.stdout,
            make_filter(Output::Stdout),
            stdout,
        ))
    }

    match &config.file_name {
        Some(file_name) if config.file.enabled => {
            let file_path = Path::new(config.file_dir.as_deref().unwrap_or(".")).join(file_name);
            let file = File::create(file_path.clone()).unwrap_or_else(|err| {
                panic!("cannot create log file: {:?}, error: {}", file_path, err)
            });
            layers.push(make_layer(
                &config.file,
                make_filter(Output::File),
                Arc::new(file),
            ))
        }
        _ => {}
    }

    if config.console {
        layers.push(
            console_subscriber::spawn()
                .with_filter(make_filter(Output::TracingConsole))
                .boxed(),
        );
    }

    tracing_subscriber::registry().with(layers).init();

    info!(
        // NOTE: intentionally logged as string and not as structured
        tracing_config = serde_json::to_string(&config).expect("cannot serialize log config"),
        "Tracing initialized"
    );
}

fn make_layer<W>(
    config: &OutputConfig,
    filter: filter::Boxed,
    writer: W,
) -> Box<dyn Layer<Registry> + Send + Sync>
where
    W: for<'writer> MakeWriter<'writer> + 'static + Send + Sync,
{
    let span_events = {
        if config.span_full {
            FmtSpan::FULL
        } else if config.span_active {
            FmtSpan::ACTIVE
        } else {
            FmtSpan::NONE
        }
    };

    if config.json {
        tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(config.json_flatten)
            .with_span_events(span_events)
            .with_writer(writer)
            .with_filter(filter)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .with_ansi(config.ansi)
            .with_span_events(span_events)
            .with_writer(writer)
            .with_filter(filter)
            .boxed()
    }
}
