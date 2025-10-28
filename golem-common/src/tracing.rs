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

use std::backtrace::Backtrace;
use std::fs::OpenOptions;
use std::io::stdout;
use std::path::Path;
use std::sync::Arc;

use figment::providers::Serialized;
use figment::Figment;
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;

use crate::config::env_config_provider;
use crate::tracing::format::JsonFlattenSpanFormatter;
use crate::SafeDisplay;

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
    pub json_flatten_span: bool,
    pub ansi: bool,
    pub compact: bool,
    pub pretty: bool,
    pub without_time: bool, // only applied for non-json
    pub span_events_active: bool,
    pub span_events_full: bool,
}

impl OutputConfig {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn text() -> Self {
        Self {
            enabled: true,
            json: false,
            json_flatten: true,
            json_flatten_span: true,
            ansi: false,
            compact: false,
            pretty: false,
            without_time: false,
            span_events_active: false,
            span_events_full: false,
        }
    }

    pub fn text_ansi() -> Self {
        Self {
            enabled: true,
            json: false,
            json_flatten: true,
            json_flatten_span: true,
            ansi: true,
            compact: false,
            pretty: false,
            without_time: false,
            span_events_active: false,
            span_events_full: false,
        }
    }

    pub fn json() -> Self {
        Self {
            enabled: true,
            json: true,
            json_flatten: false,
            json_flatten_span: false,
            ansi: false,
            compact: false,
            pretty: false,
            without_time: false,
            span_events_active: false,
            span_events_full: false,
        }
    }

    pub fn json_flatten() -> Self {
        Self {
            enabled: true,
            json: true,
            json_flatten: true,
            json_flatten_span: false,
            ansi: false,
            compact: false,
            pretty: false,
            without_time: false,
            span_events_active: false,
            span_events_full: false,
        }
    }

    pub fn json_flatten_span() -> Self {
        Self {
            enabled: true,
            json: true,
            json_flatten: true,
            json_flatten_span: true,
            ansi: false,
            compact: false,
            pretty: false,
            without_time: false,
            span_events_active: false,
            span_events_full: false,
        }
    }
}

impl SafeDisplay for OutputConfig {
    fn to_safe_string(&self) -> String {
        let mut flags = Vec::new();

        if self.ansi {
            flags.push("ansi");
        }
        if self.compact {
            flags.push("compact");
        }
        if self.json {
            flags.push("json");
        }
        if self.json_flatten {
            flags.push("json_flatten");
        }
        if self.json_flatten_span {
            flags.push("json_flatten_span");
        }
        if self.pretty {
            flags.push("pretty");
        }
        if self.without_time {
            flags.push("without_time");
        }
        if self.span_events_active {
            flags.push("span_events_active");
        }
        if self.span_events_full {
            flags.push("span_events_full");
        }

        flags.join(", ")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TracingConfig {
    pub stdout: OutputConfig,
    pub file: OutputConfig,
    pub file_dir: Option<String>,
    pub file_name: Option<String>,
    pub file_truncate: bool,
    pub console: bool,
    pub dtor_friendly: bool,
}

impl TracingConfig {
    pub fn local_dev(name: &str) -> Self {
        Self {
            stdout: OutputConfig::text_ansi(),
            file: OutputConfig {
                enabled: false,
                ..OutputConfig::json_flatten_span()
            },
            file_dir: None,
            file_name: Some(format!("{name}.log")),
            file_truncate: true,
            console: false,
            dtor_friendly: false,
        }
    }

    pub fn test(name: &str) -> Self {
        Self {
            dtor_friendly: true,
            ..Self::local_dev(name)
        }
    }

    pub fn test_pretty(name: &str) -> Self {
        let mut config = Self::test(name);
        config.stdout.pretty = true;
        config
    }

    pub fn test_pretty_without_time(name: &str) -> Self {
        let mut config = Self::test(name);
        config.stdout.pretty = true;
        config.stdout.without_time = true;
        config
    }

    pub fn test_compact(name: &str) -> Self {
        let mut config = Self::test(name);
        config.stdout.compact = true;
        config
    }

    pub fn with_env_overrides(self) -> Self {
        #[derive(Serialize, Deserialize)]
        struct Config {
            tracing: TracingConfig,
        }

        Figment::new()
            .merge(Serialized::defaults(Config { tracing: self }))
            .merge(env_config_provider())
            .extract::<Config>()
            .expect("Failed to load tracing config env overrides")
            .tracing
    }
}

impl SafeDisplay for TracingConfig {
    fn to_safe_string(&self) -> String {
        use std::fmt::Write;

        let mut result = String::new();

        if self.stdout.enabled {
            let _ = writeln!(&mut result, "stdout:");
            let _ = writeln!(&mut result, "{}", self.stdout.to_safe_string_indented());
        }
        if self.file.enabled {
            let _ = writeln!(&mut result, "file:");
            let _ = writeln!(&mut result, "{}", self.file.to_safe_string_indented());
        }
        if let Some(dir) = &self.file_dir {
            let _ = writeln!(&mut result, "file directory: {dir}");
        }
        if let Some(file) = &self.file_name {
            let _ = writeln!(&mut result, "file name: {file}");
        }
        let _ = writeln!(&mut result, "console: {}", self.console);
        let _ = writeln!(&mut result, "file truncate: {}", self.file_truncate);
        let _ = writeln!(&mut result, "destructor friendly: {}", self.dtor_friendly);

        result
    }
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            stdout: OutputConfig::json_flatten_span(),
            file: OutputConfig {
                enabled: false,
                ..OutputConfig::json_flatten_span()
            },
            file_dir: None,
            file_name: None,
            file_truncate: true,
            console: false,
            dtor_friendly: false,
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
        format!("{target}=debug").parse().unwrap()
    }

    pub fn info(target: &str) -> Directive {
        format!("{target}=info").parse().unwrap()
    }

    pub fn warn(target: &str) -> Directive {
        format!("{target}=warn").parse().unwrap()
    }

    pub fn error(target: &str) -> Directive {
        format!("{target}=error").parse().unwrap()
    }

    pub fn default_deps() -> Vec<Directive> {
        vec![
            warn("cranelift_codegen"),
            warn("wasmtime_cranelift"),
            warn("wasmtime_jit"),
            warn("h2"),
            warn("hyper"),
            warn("tower"),
            error("fred"),
            warn("wac_graph"),
            warn("wasmtime_environ"),
            warn("wit_parser"),
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

pub fn init_tracing<F>(config: &TracingConfig, make_filter: F)
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

            let mut open_options = OpenOptions::new();
            if config.file_truncate {
                open_options.write(true).create(true).truncate(true);
            } else {
                open_options.append(true).create(true).truncate(false);
            }

            let file = open_options.open(&file_path).unwrap_or_else(|err| {
                panic!("cannot create log file: {:?}, error: {}", &file_path, err)
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

    std::panic::set_hook({
        Box::new(|panic_info| {
            error!(panic_info = %panic_info, panic_backtrace=%Backtrace::force_capture() , "panic");
        })
    });

    if !config.dtor_friendly {
        info!(
            // NOTE: intentionally logged as string and not as structured
            tracing_config = serde_json::to_string(&config).expect("cannot serialize log config"),
            "Tracing initialized"
        );
    }
}

pub fn init_tracing_with_default_env_filter(config: &TracingConfig) {
    init_tracing(config, filter::for_all_outputs::DEFAULT_ENV);
}

pub fn init_tracing_with_default_debug_env_filter(config: &TracingConfig) {
    init_tracing(config, filter::for_all_outputs::default_debug_env());
}

#[allow(clippy::collapsible_else_if)]
fn make_layer<W>(
    config: &OutputConfig,
    filter: filter::Boxed,
    writer: W,
) -> Box<dyn Layer<Registry> + Send + Sync>
where
    W: for<'writer> MakeWriter<'writer> + 'static + Send + Sync,
{
    let span_events = {
        if config.span_events_full {
            FmtSpan::FULL
        } else if config.span_events_active {
            FmtSpan::ACTIVE
        } else {
            FmtSpan::NONE
        }
    };

    if config.json {
        if config.json_flatten_span {
            tracing_subscriber::fmt::layer()
                .json() // for setting the field formatter
                .with_span_events(span_events)
                .event_format(JsonFlattenSpanFormatter)
                .with_writer(writer)
                .with_filter(filter)
                .boxed()
        } else {
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(config.json_flatten)
                .with_span_events(span_events)
                .with_writer(writer)
                .with_filter(filter)
                .boxed()
        }
    } else {
        let layer = tracing_subscriber::fmt::layer()
            .with_ansi(config.ansi)
            .with_span_events(span_events)
            .with_writer(writer);

        let layer = if config.pretty {
            if config.without_time {
                layer.pretty().without_time().boxed()
            } else {
                layer.pretty().boxed()
            }
        } else if config.compact {
            if config.without_time {
                layer.compact().without_time().boxed()
            } else {
                layer.compact().boxed()
            }
        } else {
            if config.without_time {
                layer.without_time().boxed()
            } else {
                layer.boxed()
            }
        };

        layer.with_filter(filter).boxed()
    }
}

pub(crate) mod format {
    use std::collections::BTreeSet;
    use std::{fmt, io};

    use serde::ser::{SerializeMap, Serializer as _};
    use serde_json::value::RawValue;
    use serde_json::Serializer;
    use tracing::{Event, Subscriber};
    use tracing_serde::AsSerde;
    use tracing_subscriber::fmt::format::Writer;
    use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
    use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, FormattedFields};
    use tracing_subscriber::registry::LookupSpan;

    pub struct JsonFlattenSpanFormatter;

    // Based on `impl<S, N, T> FormatEvent<S, N> for Format<Json, T>`
    impl<S, N> FormatEvent<S, N> for JsonFlattenSpanFormatter
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
        N: for<'writer> FormatFields<'writer> + 'static,
    {
        fn format_event(
            &self,
            ctx: &FmtContext<'_, S, N>,
            mut writer: Writer<'_>,
            event: &Event<'_>,
        ) -> fmt::Result
        where
            S: Subscriber + for<'a> LookupSpan<'a>,
        {
            let mut timestamp = String::new();
            SystemTime.format_time(&mut Writer::new(&mut timestamp))?;

            let meta = event.metadata();

            let mut visit = || {
                let mut serializer = Serializer::new(WriteAdaptor::new(&mut writer));
                let mut serializer = serializer.serialize_map(None)?;

                serializer.serialize_entry("timestamp", &timestamp)?;
                serializer.serialize_entry("level", &meta.level().as_serde())?;
                serializer.serialize_entry("target", meta.target())?;

                let mut visitor = tracing_serde::SerdeMapVisitor::new(serializer);
                event.record(&mut visitor);

                serializer = visitor.take_serializer()?;

                let mut spans = BTreeSet::new();
                if let Some(span) = ctx.lookup_current() {
                    for span in span.scope() {
                        if spans.contains(span.name()) {
                            continue;
                        }
                        spans.insert(span.name());

                        let extensions = span.extensions();
                        let data = extensions
                            .get::<FormattedFields<N>>()
                            .expect("Unable to find FormattedFields in extensions");
                        let raw_data = RawValue::from_string(data.as_str().to_owned())
                            .expect("Unable to read fields as RawValue");

                        serializer.serialize_entry(span.name(), &raw_data)?
                    }
                }

                SerializeMap::end(serializer)
            };

            visit().map_err(|_| fmt::Error)?;
            writeln!(writer)
        }
    }

    // From tracing_subscriber::fmt::writer::WriteAdaptor
    struct WriteAdaptor<'a> {
        fmt_write: &'a mut dyn fmt::Write,
    }

    impl<'a> WriteAdaptor<'a> {
        pub fn new(fmt_write: &'a mut dyn fmt::Write) -> Self {
            Self { fmt_write }
        }
    }

    impl io::Write for WriteAdaptor<'_> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let s = std::str::from_utf8(buf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            self.fmt_write.write_str(s).map_err(io::Error::other)?;

            Ok(s.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod test {
    pub fn make_mock_writer<'a>() -> tracing_test::internal::MockWriter<'a> {
        tracing_test::internal::MockWriter::new(tracing_test::internal::global_buf())
    }

    pub fn get_logs() -> String {
        String::from_utf8(
            tracing_test::internal::global_buf()
                .lock()
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    mod json_flatten_span_formatter {
        use test_r::test;

        use tracing;
        use tracing::{field, info, span, Level};
        use tracing_subscriber::FmtSubscriber;

        use crate::tracing::format::JsonFlattenSpanFormatter;
        use crate::tracing::test::{get_logs, make_mock_writer};

        #[test]
        fn json_flatten_span_formatter_duplicated_spans_are_removed() {
            let writer = make_mock_writer();
            let subscriber = FmtSubscriber::builder()
                .json()
                .flatten_event(true)
                .event_format(JsonFlattenSpanFormatter)
                .with_writer(writer)
                .finish();

            tracing::subscriber::with_default(subscriber, || {
                const SPAN_NAME: &str = "custom_span";
                let span1 = span!(Level::INFO, SPAN_NAME, span_prop = field::Empty);
                let _enter = span1.enter();
                span1.record("span_prop", "value_1");
                span1.record("span_prop", "value_2");

                let span2 = span!(Level::INFO, SPAN_NAME, span_prop = field::Empty);
                let _enter = span2.enter();
                span2.record("span_prop", "value_3");
                span2.record("span_prop", "value_4");

                info!(value = "value", "hello");
            });

            let logs = get_logs();

            assert_eq!(logs.matches("\"custom_span\"").count(), 1);
            assert_eq!(logs.matches("\"span_prop\"").count(), 1);
            assert_eq!(logs.matches("\"value_1\"").count(), 0);
            assert_eq!(logs.matches("\"value_2\"").count(), 0);
            assert_eq!(logs.matches("\"value_3\"").count(), 0);
            assert_eq!(logs.matches("\"value_4\"").count(), 1);
        }
    }
}
