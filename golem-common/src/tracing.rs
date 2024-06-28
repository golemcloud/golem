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
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::debug;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Filter, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
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
    #[allow(dead_code)]
    pub fn disabled() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Config {
    pub stdout: OutputConfig,
    pub file: OutputConfig,
    pub file_path: Option<String>,
    pub tracing_console: bool,
}

impl Config {
    #[allow(dead_code)]
    pub fn local_dev(service_name: &str) -> Self {
        Self {
            stdout: OutputConfig::text_ansi(),
            file: OutputConfig::text(),
            file_path: Some(format!("../logs/{}.log", service_name)),
            tracing_console: false,
        }
    }
}

pub fn default_env_filter(_output: Output) -> Box<dyn Filter<Registry> + 'static + Send + Sync> {
    Box::new(EnvFilter::from_default_env())
}

pub fn init<F>(config: &Config, make_filter: F)
where
    F: Fn(Output) -> Box<dyn Filter<Registry> + 'static + Send + Sync>,
{
    let mut layers = Vec::new();

    if config.stdout.enabled {
        layers.push(make_layer(
            &config.stdout,
            make_filter(Output::Stdout),
            stdout,
        ))
    }

    match config.file_path {
        Some(ref file_path) if config.file.enabled => {
            let file = File::create(file_path).unwrap_or_else(|err| {
                panic!("cannot create log file: {}, error: {}", file_path, err)
            });
            layers.push(make_layer(
                &config.file,
                make_filter(Output::File),
                Arc::new(file),
            ))
        }
        _ => {}
    }

    if config.tracing_console {
        layers.push(
            console_subscriber::spawn()
                .with_filter(make_filter(Output::TracingConsole))
                .boxed(),
        );
    }

    tracing_subscriber::registry().with(layers).init();

    debug!(
        // NOTE: intentionally logged as string and not as structured
        tracing_config = serde_json::to_string(&config).expect("cannot serialize log config"),
        "Tracing inited"
    );
}

fn make_layer<W>(
    config: &OutputConfig,
    filter: Box<dyn Filter<Registry> + 'static + Send + Sync>,
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
