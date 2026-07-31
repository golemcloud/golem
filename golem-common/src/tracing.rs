// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use std::io::{stderr, stdout};
use std::path::Path;
use std::sync::Arc;

use figment::Figment;
use figment::providers::Serialized;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracer;
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::SafeDisplay;
use crate::config::env_config_provider;
use crate::tracing::format::JsonFlattenSpanFormatter;

pub use origin::{SPAN_TARGET, TraceOrigin};

mod origin;

pub enum Output {
    Stdout,
    Stderr,
    File,
    TracingConsole,
    Otlp,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct OutputConfig {
    pub enabled: bool,
    pub json: bool,
    pub json_flatten: bool,
    pub json_flatten_span: bool,
    pub json_source_location: bool,
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
            json_source_location: false,
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
            json_source_location: false,
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
            json_source_location: false,
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
            json_source_location: false,
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
            json_source_location: false,
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
        if self.json_source_location {
            flags.push("json_source_location");
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
    pub stderr: OutputConfig,
    pub file: OutputConfig,
    pub file_dir: Option<String>,
    pub file_name: Option<String>,
    pub file_truncate: bool,
    pub console: bool,
    pub dtor_friendly: bool,
    pub otlp: OtlpConfig,
}

impl TracingConfig {
    pub fn local_dev(name: &str) -> Self {
        Self {
            stdout: OutputConfig::text_ansi(),
            stderr: OutputConfig::disabled(),
            file: OutputConfig {
                enabled: false,
                ..OutputConfig::json_flatten_span()
            },
            file_dir: None,
            file_name: Some(format!("{name}.log")),
            file_truncate: true,
            console: false,
            dtor_friendly: false,
            otlp: OtlpConfig::default(),
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

    pub fn with_otlp(mut self, enabled: bool, host: &str, port: u16, service_name: &str) -> Self {
        self.otlp = OtlpConfig {
            enabled,
            host: host.to_string(),
            port,
            service_name: service_name.to_string(),
        };
        self
    }

    pub fn use_stderr(mut self) -> Self {
        self.stderr = self.stdout.clone();
        self.stdout.enabled = false;
        self
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
        if self.otlp.enabled {
            let _ = writeln!(&mut result, "otlp:");
            let _ = writeln!(&mut result, "{}", self.otlp.to_safe_string_indented());
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
            stderr: OutputConfig::disabled(),
            file: OutputConfig {
                enabled: false,
                ..OutputConfig::json_flatten_span()
            },
            file_dir: None,
            file_name: None,
            file_truncate: true,
            console: false,
            dtor_friendly: false,
            otlp: OtlpConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtlpConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub service_name: String,
}

impl SafeDisplay for OtlpConfig {
    fn to_safe_string(&self) -> String {
        use std::fmt::Write;

        let mut result = String::new();

        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "service_name: {}", self.service_name);

        result
    }
}

impl Default for OtlpConfig {
    fn default() -> Self {
        OtlpConfig {
            enabled: false,
            host: "localhost".to_string(),
            port: 4318,
            service_name: "golem".to_string(),
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

    pub fn off(target: &str) -> Directive {
        format!("{target}=off").parse().unwrap()
    }

    pub fn default_deps() -> Vec<Directive> {
        vec![
            warn("cranelift_codegen"),
            warn("wasmtime_cranelift"),
            warn("wasmtime_internal_cranelift"),
            warn("wasmtime_jit"),
            warn("h2"),
            warn("hyper"),
            warn("tower"),
            error("fred"),
            warn("wac_graph"),
            warn("wasmtime_environ"),
            warn("wit_parser"),
            warn("golem_client"),
            warn("bollard"),
        ]
    }

    /// Directives for the OTLP layer: same as default_deps but also
    /// enables trace-level for `otel::tracing` target so that spans
    /// created by `tonic-tracing-opentelemetry` (which uses TRACE level)
    /// are exported and can propagate trace context across services.
    ///
    /// These targets emit events far more often than once per operation - the WASM
    /// host-call implementations (one or more per host call, including
    /// guest-controlled log output) and the RDBMS layer (per query and per row as
    /// the guest pulls results) - and every one of them records onto the enclosing
    /// invocation span, which stays open for as long as the invocation runs.
    ///
    /// `tracing-opentelemetry` holds an event for as long as its span is open, and
    /// the SDK applies `max_events_per_span` only at span close, keeping the *first*
    /// N. So a long invocation accumulates every event it emits and exports almost
    /// none of them. Silencing these targets for the OTLP layer is what keeps that
    /// bounded; console and file output still log them.
    ///
    /// This is a default, not a rule: a `GOLEM_OTLP_LOG` directive naming one of
    /// these targets wins over it, which is how you get RDBMS or host-call detail
    /// into a trace when chasing a specific bottleneck.
    fn otlp_silenced_targets() -> Vec<Directive> {
        vec![
            off("golem_worker_executor::durable_host"),
            off("golem_worker_executor::services::rdbms"),
        ]
    }

    pub fn otlp_deps() -> Vec<Directive> {
        let mut deps = default_deps();
        deps.push("otel::tracing=trace".parse().unwrap());
        deps.extend(otlp_silenced_targets());
        deps
    }

    /// The full directive list for the OTLP layer, as an `EnvFilter` spec.
    ///
    /// `user` is the raw `GOLEM_OTLP_LOG` value. It is appended last because
    /// `EnvFilter` lets a later directive replace an earlier one for the same
    /// target, so every default here stays overridable per target.
    pub fn otlp_spec(user: Option<&str>) -> String {
        let mut spec = default::info().to_string();
        for directive in otlp_deps() {
            spec.push(',');
            spec.push_str(&directive.to_string());
        }
        if let Some(user) = user.map(str::trim).filter(|user| !user.is_empty()) {
            spec.push(',');
            spec.push_str(user);
        }
        spec
    }
}

pub mod filter {
    use tracing_subscriber::Registry;
    use tracing_subscriber::layer::Filter;

    pub type Boxed = Box<dyn Filter<Registry> + 'static + Send + Sync>;

    pub mod boxed {
        use tracing_subscriber::EnvFilter;
        use tracing_subscriber::filter::Directive;

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

        /// Filter for the OTLP layer: info level by default, with
        /// `otel::tracing=trace` so context-propagation spans are exported.
        ///
        /// Info is the level at which spans describe whole operations - requests,
        /// invocations, worker admission - which is what a trace is read for. The
        /// detail below that (per-host-call retries, background loop ticks) is at
        /// debug, so raising the level deepens a trace rather than changing what it
        /// is about.
        ///
        /// Unlike the other filters this reads `GOLEM_OTLP_LOG` rather than
        /// `RUST_LOG`, because `RUST_LOG` controls console verbosity and is often set
        /// to `warn` in benchmark and CI runs, which would silently suppress every
        /// exported span. Its value is applied on top of the defaults in
        /// [`directive::otlp_spec`], so it can raise or lower any individual target.
        pub fn default_otlp_env() -> Boxed {
            let user = std::env::var("GOLEM_OTLP_LOG").ok();
            Box::new(EnvFilter::builder().parse_lossy(directive::otlp_spec(user.as_deref())))
        }
    }

    pub mod for_all_outputs {
        use tracing_subscriber::filter::Directive;

        use crate::tracing::Output;
        use crate::tracing::filter::{Boxed, boxed};

        /// For OTLP, uses a permissive debug-level filter with
        /// `otel::tracing=trace` so that context-propagation spans created
        /// by `tonic-tracing-opentelemetry` (at TRACE level) are always
        /// exported. For other outputs falls back to the RUST_LOG-based
        /// env filter.
        pub const DEFAULT_ENV: fn(Output) -> Boxed = |output| match output {
            Output::Otlp => boxed::default_otlp_env(),
            _ => boxed::default_env(),
        };

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

pub fn init_tracing<F>(config: &TracingConfig, make_filter: F) -> Option<SdkTracer>
where
    F: Fn(Output) -> filter::Boxed,
{
    init_tracing_returning_provider(config, make_filter).map(|(tracer, _provider)| tracer)
}

/// Like [`init_tracing`] but also returns the [`SdkTracerProvider`] so the
/// caller can call [`SdkTracerProvider::shutdown`] before process exit to
/// flush pending OTLP spans.
pub fn init_tracing_returning_provider<F>(
    config: &TracingConfig,
    make_filter: F,
) -> Option<(SdkTracer, opentelemetry_sdk::trace::SdkTracerProvider)>
where
    F: Fn(Output) -> filter::Boxed,
{
    let mut layers = Vec::new();
    let mut result = None;

    if config.otlp.enabled {
        let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(format!(
                "http://{}:{}/v1/traces",
                config.otlp.host, config.otlp.port
            ))
            .build()
            .expect("Failed to build OTLP exporter");

        let resource = Resource::builder()
            .with_service_name(config.otlp.service_name.clone())
            .build();

        let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(otlp_exporter)
            .build();

        global::set_text_map_propagator(TraceContextPropagator::new());

        let tracer = tracer_provider.tracer(config.otlp.service_name.clone());
        let telemetry = tracing_opentelemetry::layer().with_tracer(tracer.clone());
        result = Some((tracer, tracer_provider));

        layers.push(telemetry.with_filter(make_filter(Output::Otlp)).boxed());
    }

    if config.stdout.enabled {
        layers.push(make_layer(
            &config.stdout,
            make_filter(Output::Stdout),
            stdout,
        ))
    }

    if config.stderr.enabled {
        layers.push(make_layer(
            &config.stdout,
            make_filter(Output::Stderr),
            stderr,
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

    result
}

pub fn init_tracing_with_default_env_filter(config: &TracingConfig) -> Option<SdkTracer> {
    init_tracing(config, filter::for_all_outputs::DEFAULT_ENV)
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
                .with_file(config.json_source_location)
                .with_line_number(config.json_source_location)
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
    use serde_json::Serializer;
    use serde_json::value::RawValue;
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
pub(crate) mod test {
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

    /// Support for asserting on what the OTLP path actually exports.
    ///
    /// Tests here run against a real `tracing-opentelemetry` layer and a real
    /// `SdkTracerProvider`, because what is being verified is precisely how that
    /// stack treats parents, links, span kind and span lifetime - a hand-written
    /// fake would verify nothing.
    pub(crate) mod otel {
        use std::sync::{Arc, Mutex};

        use opentelemetry::trace::TracerProvider;
        use opentelemetry_sdk::error::OTelSdkResult;
        use opentelemetry_sdk::trace::{
            SdkTracerProvider, SimpleSpanProcessor, SpanData, SpanExporter,
        };
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Debug, Clone, Default)]
        struct CollectingExporter {
            spans: Arc<Mutex<Vec<SpanData>>>,
        }

        impl SpanExporter for CollectingExporter {
            fn export(
                &self,
                batch: Vec<SpanData>,
            ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
                self.spans.lock().unwrap().extend(batch);
                std::future::ready(Ok(()))
            }
        }

        /// Runs `f` under a subscriber whose only layer is a `tracing-opentelemetry`
        /// layer, and returns the exported spans.
        ///
        /// `SimpleSpanProcessor` exports synchronously when a span closes, so every
        /// span closed inside `f` is collected by the time this returns. A span
        /// still open at the end of `f` is absent, which is what makes this usable
        /// for asserting on span lifetime.
        pub(crate) fn exported_spans(f: impl FnOnce()) -> Vec<SpanData> {
            let collected: Arc<Mutex<Vec<SpanData>>> = Arc::new(Mutex::new(Vec::new()));
            let provider = SdkTracerProvider::builder()
                .with_span_processor(SimpleSpanProcessor::new(CollectingExporter {
                    spans: collected.clone(),
                }))
                .build();
            let subscriber = tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));

            tracing::subscriber::with_default(subscriber, f);

            collected.lock().unwrap().clone()
        }

        /// As [`exported_spans`], but with `filter` applied to the OTLP layer, so
        /// what reaches the exporter is what the real OTLP layer would export.
        pub(crate) fn exported_spans_filtered(
            filter: crate::tracing::filter::Boxed,
            f: impl FnOnce(),
        ) -> Vec<SpanData> {
            use tracing_subscriber::Layer;

            let collected: Arc<Mutex<Vec<SpanData>>> = Arc::new(Mutex::new(Vec::new()));
            let provider = SdkTracerProvider::builder()
                .with_span_processor(SimpleSpanProcessor::new(CollectingExporter {
                    spans: collected.clone(),
                }))
                .build();
            let subscriber = tracing_subscriber::registry().with(
                tracing_opentelemetry::layer()
                    .with_tracer(provider.tracer("test"))
                    .with_filter(filter),
            );

            tracing::subscriber::with_default(subscriber, f);

            collected.lock().unwrap().clone()
        }

        pub(crate) fn named<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
            spans.iter().find(|s| s.name == name).unwrap_or_else(|| {
                let found: Vec<&str> = spans.iter().map(|s| &*s.name).collect();
                panic!("no exported span named {name:?}; exported: {found:?}")
            })
        }
    }

    /// Tests for the OTLP layer's directive set.
    mod otlp_spec {
        use test_r::test;
        use tracing_subscriber::EnvFilter;

        use crate::tracing::SPAN_TARGET;
        use crate::tracing::directive::otlp_spec;
        use crate::tracing::test::otel::{exported_spans_filtered, named};

        const HOST_CALL: &str = "golem_worker_executor::durable_host::http";
        const RDBMS: &str = "golem_worker_executor::services::rdbms::postgres";

        /// The span is deliberately still open while the events are emitted, since
        /// that is the state in which they accumulate.
        fn events_on_span(user: Option<&str>) -> usize {
            let filter = EnvFilter::builder().parse_lossy(otlp_spec(user));
            let spans = exported_spans_filtered(Box::new(filter), || {
                let span = tracing::info_span!("invocation");
                let _guard = span.enter();
                tracing::error!(target: HOST_CALL, "host call failed");
                tracing::info!(target: HOST_CALL, "host call");
                tracing::info!(target: RDBMS, "query");
            });
            named(&spans, "invocation").events.len()
        }

        #[test]
        fn high_volume_targets_contribute_no_events_by_default() {
            assert_eq!(events_on_span(None), 0);
        }

        /// The deployed setting, which must not weaken the default.
        #[test]
        fn a_bare_level_does_not_re_enable_them() {
            assert_eq!(events_on_span(Some("info")), 0);
            assert_eq!(events_on_span(Some("debug")), 0);
        }

        #[test]
        fn naming_a_target_explicitly_re_enables_it() {
            assert_eq!(
                events_on_span(Some("golem_worker_executor::services::rdbms=info")),
                1
            );
            assert_eq!(
                events_on_span(Some(
                    "golem_worker_executor::durable_host=info,golem_worker_executor::services::rdbms=info"
                )),
                3
            );
        }

        /// Silencing a module must stop its events without taking its spans with
        /// them: the spans are bounded and are the thing worth exporting, and they
        /// are declared under [`SPAN_TARGET`] precisely so a directive aimed at the
        /// module cannot reach them.
        ///
        /// Covers both routes into a span, since a plain `span!` in such a module is
        /// just as easy to write as a `related_span!` and just as easily silenced.
        #[test]
        fn a_span_declared_in_a_silenced_module_still_exports() {
            let filter = EnvFilter::builder().parse_lossy(otlp_spec(None));
            let spans = exported_spans_filtered(Box::new(filter), || {
                // As if written inside the silenced module, which is where the
                // host-call retry and replay spans live.
                let origin = crate::tracing::TraceOrigin::none();
                let linked = crate::related_span!(origin, tracing::Level::INFO, "host_call_retry");
                linked.in_scope(|| {
                    tracing::info!(target: HOST_CALL, "silenced host call event");
                });

                tracing::info_span!(target: SPAN_TARGET, "replaying").in_scope(|| {});
            });

            assert!(
                named(&spans, "host_call_retry").events.is_empty(),
                "the module's events stay silenced even inside its own span"
            );
            named(&spans, "replaying");
        }

        /// The mistake the target exists to prevent, pinned so it stays visible: a
        /// span left on its module's own target is silenced along with its events.
        #[test]
        fn a_span_left_on_a_silenced_modules_target_does_not_export() {
            let filter = EnvFilter::builder().parse_lossy(otlp_spec(None));
            let spans = exported_spans_filtered(Box::new(filter), || {
                tracing::info_span!(target: HOST_CALL, "forgot_the_span_target").in_scope(|| {});
            });

            assert!(spans.is_empty());
        }

        #[test]
        fn events_from_other_targets_are_untouched() {
            let filter = EnvFilter::builder().parse_lossy(otlp_spec(None));
            let spans = exported_spans_filtered(Box::new(filter), || {
                let span = tracing::info_span!("invocation");
                let _guard = span.enter();
                tracing::info!(target: "golem_worker_executor::services::worker", "unrelated");
            });
            assert_eq!(named(&spans, "invocation").events.len(), 1);
        }

        #[test]
        fn the_default_level_is_info() {
            let filter = EnvFilter::builder().parse_lossy(otlp_spec(None));
            let spans = exported_spans_filtered(Box::new(filter), || {
                tracing::info_span!("kept").in_scope(|| {});
                tracing::debug_span!("dropped").in_scope(|| {});
            });
            named(&spans, "kept");
            assert!(spans.iter().all(|s| s.name != "dropped"));
        }

        #[test]
        fn an_unparseable_user_directive_does_not_discard_the_defaults() {
            let filter = EnvFilter::builder().parse_lossy(otlp_spec(Some("not a level")));
            let spans = exported_spans_filtered(Box::new(filter), || {
                let span = tracing::info_span!("invocation");
                let _guard = span.enter();
                tracing::info!(target: HOST_CALL, "host call");
            });
            assert_eq!(named(&spans, "invocation").events.len(), 0);
        }
    }

    /// Tests for [`crate::tracing::TraceOrigin`].
    mod trace_origin {
        use opentelemetry::trace::SpanId;
        use test_r::test;

        use crate::tracing::TraceOrigin;
        use crate::tracing::test::otel::{exported_spans, named};

        /// Work is handed off, so execution is a new trace linked back to whatever
        /// enqueued it. Per the OpenTelemetry messaging conventions the link lives
        /// on the consumer span.
        #[test]
        fn add_as_link_to_starts_a_new_trace_linked_to_the_captured_span() {
            let spans = exported_spans(|| {
                let enqueue = tracing::info_span!("enqueue");
                let captured = enqueue.in_scope(TraceOrigin::capture_current);
                // The enqueuing request returns before the work is picked up.
                drop(enqueue);

                let picked_up = tracing::info_span!(parent: None, "picked_up");
                captured.add_as_link_to(&picked_up);
                drop(picked_up);
            });

            let enqueue = named(&spans, "enqueue");
            let picked_up = named(&spans, "picked_up");

            assert_ne!(
                picked_up.span_context.trace_id(),
                enqueue.span_context.trace_id(),
                "linked work should be a separate trace"
            );
            assert_eq!(
                picked_up.parent_span_id,
                SpanId::INVALID,
                "linked work should be a trace root, not a child"
            );

            let link_targets: Vec<(_, _)> = picked_up
                .links
                .iter()
                .map(|l| (l.span_context.trace_id(), l.span_context.span_id()))
                .collect();
            assert!(
                link_targets.contains(&(
                    enqueue.span_context.trace_id(),
                    enqueue.span_context.span_id()
                )),
                "consumer span should link to the enqueuing span; links were {link_targets:?}"
            );
        }

        /// Capturing a parent must not keep the captured span open, so that a span
        /// remembered for later use still closes and exports on its own schedule.
        #[test]
        fn capturing_a_parent_does_not_keep_the_captured_span_open() {
            let mut held: Option<TraceOrigin> = None;

            let spans = exported_spans(|| {
                let request = tracing::info_span!("request");
                held = Some(request.in_scope(TraceOrigin::capture_current));
                drop(request);
                // `held` is still alive here, as the pending-invocation map holds it
                // while the invocation waits to be picked up.
            });

            assert!(held.is_some_and(|parent| !parent.is_empty()));
            assert_eq!(
                spans.len(),
                1,
                "the captured span must still close and export while its TraceOrigin is held"
            );
            assert_eq!(spans[0].name, "request");
        }

        /// With no active span, or with no OpenTelemetry layer installed, a captured
        /// parent is empty and applying it changes nothing.
        #[test]
        fn an_empty_parent_leaves_the_span_as_a_root() {
            let empty = TraceOrigin::default();
            assert!(empty.is_empty());

            let spans = exported_spans(|| {
                let root = tracing::info_span!(parent: None, "root");
                empty.add_as_link_to(&root);
                drop(root);
            });

            let root = named(&spans, "root");
            assert_eq!(root.parent_span_id, SpanId::INVALID);
            assert!(root.links.is_empty());
        }

        /// Handed-off work is a linked root, not a child - including the invocation
        /// path where the caller waits for the result, since the worker runs it
        /// independently and the caller is only notified.
        #[test]
        fn related_span_makes_the_work_a_linked_root() {
            let spans = exported_spans(|| {
                let caller = tracing::info_span!("caller");
                let origin = caller.in_scope(TraceOrigin::capture_current);

                let work = crate::related_span!(origin, tracing::Level::INFO, "work");
                drop(work);
                drop(caller);
            });

            let caller = named(&spans, "caller");
            let work = named(&spans, "work");
            assert_ne!(
                work.span_context.trace_id(),
                caller.span_context.trace_id(),
                "linked work is the root of its own trace"
            );
            assert_eq!(work.parent_span_id, SpanId::INVALID);
            assert_eq!(work.links.iter().count(), 1);
            assert_eq!(
                work.links.iter().next().unwrap().span_context.span_id(),
                caller.span_context.span_id(),
                "the link points back at whatever handed the work off"
            );
        }

        #[test]
        fn capture_current_outside_any_span_is_empty() {
            let mut captured = None;
            exported_spans(|| {
                captured = Some(TraceOrigin::capture_current());
            });
            assert!(captured.is_some_and(|parent| parent.is_empty()));
        }
    }

    mod json_flatten_span_formatter {
        use test_r::test;

        use tracing;
        use tracing::{Level, field, info, span};
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
