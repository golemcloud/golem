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

use test_r::test;

#[test]
fn standard_streams_keep_their_own_settings_filters_and_writers() {
    use super::{Output, OutputConfig, TracingConfig, stdio_layers};
    use std::sync::Arc;
    use tracing_subscriber::layer::SubscriberExt;

    let assert_outputs = |stdout: OutputConfig, stderr: OutputConfig| {
        let config = TracingConfig {
            stdout,
            stderr,
            ..TracingConfig::local_dev("stdio-settings")
        };
        let stdout_file = tempfile::NamedTempFile::new().unwrap();
        let stderr_file = tempfile::NamedTempFile::new().unwrap();
        let filters = |output| -> super::filter::Boxed {
            let target = match output {
                Output::Stdout => {
                    assert!(config.stdout.enabled);
                    "stdio_stdout_probe"
                }
                Output::Stderr => {
                    assert!(config.stderr.enabled);
                    "stdio_stderr_probe"
                }
                _ => panic!("only standard-stream filters are needed"),
            };
            Box::new(tracing_subscriber::filter::filter_fn(move |metadata| {
                metadata.target() == target
            }))
        };
        let layers = stdio_layers(
            &config,
            &filters,
            Arc::new(stdout_file.reopen().unwrap()),
            Arc::new(stderr_file.reopen().unwrap()),
        )
        .collect::<Vec<_>>();
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layers), || {
            tracing::info!(target: "stdio_stdout_probe", "stdout-marker");
            tracing::info!(target: "stdio_stderr_probe", "stderr-marker");
            tracing::info!(target: "stdio_rejected_probe", "rejected-marker");
        });
        [
            (
                &stdout_file,
                &config.stdout,
                "stdout-marker",
                "stderr-marker",
            ),
            (
                &stderr_file,
                &config.stderr,
                "stderr-marker",
                "stdout-marker",
            ),
        ]
        .into_iter()
        .for_each(|(file, output, accepted, rejected)| {
            let contents = std::fs::read_to_string(file.path()).unwrap();
            if !output.enabled {
                assert!(contents.is_empty(), "disabled stream: {contents}");
                return;
            }
            assert!(
                contents.contains(accepted),
                "missing {accepted}: {contents}"
            );
            assert!(!contents.contains(rejected), "wrong stream: {contents}");
            assert!(
                !contents.contains("rejected-marker"),
                "unfiltered: {contents}"
            );
            let json = serde_json::from_str::<serde_json::Value>(&contents);
            if output.json {
                let json = json.expect("the stream must use its own JSON format");
                let message = if output.json_flatten {
                    &json["message"]
                } else {
                    &json["fields"]["message"]
                };
                assert_eq!(message, accepted);
                assert_eq!(json.get("filename").is_some(), output.json_source_location);
                assert_eq!(
                    json.get("line_number").is_some(),
                    output.json_source_location
                );
            } else {
                assert!(json.is_err(), "the stream must use its own text format");
            }
        });
    };

    [(true, true), (true, false), (false, true), (false, false)]
        .into_iter()
        .for_each(|(stdout_enabled, stderr_enabled)| {
            [
                (OutputConfig::text(), OutputConfig::json()),
                (OutputConfig::json(), OutputConfig::text()),
            ]
            .into_iter()
            .for_each(|(mut stdout, mut stderr)| {
                stdout.enabled = stdout_enabled;
                stderr.enabled = stderr_enabled;
                assert_outputs(stdout, stderr);
            });
        });
    [false, true].into_iter().for_each(|source_location| {
        let stdout = OutputConfig {
            json_source_location: source_location,
            json_flatten: source_location,
            ..OutputConfig::json()
        };
        let stderr = OutputConfig {
            json_source_location: !source_location,
            json_flatten: !source_location,
            ..OutputConfig::json()
        };
        assert_outputs(stdout, stderr);
    });
}

#[test]
fn tracing_env_overrides_use_surface_and_server_advanced_settings() {
    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("GOLEM__LOGGING__STDOUT__ENABLED", false);
        jail.set_env("GOLEM__LOGGING__STDERR__ENABLED", true);
        jail.set_env("GOLEM__OTLP__ENABLED", true);
        jail.set_env("GOLEM__OTLP__HOST", "surface-collector");
        jail.set_env("GOLEM__OTLP__PORT", 7321);
        jail.set_env("GOLEM__OTLP__SERVICE_NAME", "surface-service");
        let mut base = super::TracingConfig::local_dev("retained-base");
        base.file_truncate = false;

        let config = base.clone().with_env_overrides();
        assert!(!config.stdout.enabled);
        assert!(config.stderr.enabled);
        assert!(config.otlp.enabled);
        assert_eq!(config.otlp.host, "surface-collector");
        assert_eq!(config.otlp.port, 7321);
        assert_eq!(config.otlp.service_name, "surface-service");
        assert_eq!(config.file_name.as_deref(), Some("retained-base.log"));
        assert!(!config.file_truncate);

        jail.set_env("GOLEM_ADVANCED__SERVER__TRACING__OTLP__PORT", 8456);
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_SERVICE__TRACING__OTLP__HOST",
            "must-not-reach-server",
        );
        let config = base.with_env_overrides();
        assert_eq!(config.otlp.port, 8456);
        assert_eq!(config.otlp.host, "surface-collector");
        assert!(!config.stdout.enabled);
        assert!(config.stderr.enabled);
        assert!(!config.file_truncate);
        Ok(())
    });
}

#[test]
fn propagation_without_export_preserves_parent_with_logging_disabled() {
    use opentelemetry::propagation::TextMapPropagator;
    use opentelemetry::trace::TraceContextExt;
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use tracing::Subscriber;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt};

    struct DiscardedPayload;
    impl std::fmt::Debug for DiscardedPayload {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("disabled log event must not format its payload");
        }
    }

    let subscriber = tracing_subscriber::registry().with(vec![
        super::propagation_layer().boxed(),
        tracing_subscriber::fmt::layer()
            .with_filter(EnvFilter::new("off"))
            .boxed(),
    ]);
    assert_eq!(
        subscriber.max_level_hint(),
        Some(tracing_subscriber::filter::LevelFilter::INFO),
        "propagation must not enable verbose log-facade argument evaluation"
    );
    tracing::subscriber::with_default(subscriber, || {
        for sampled in ["00", "01"] {
            let headers = std::collections::HashMap::from([(
                "traceparent".to_string(),
                format!("00-0123456789abcdef0123456789abcdef-1234567890abcdef-{sampled}"),
            )]);
            let parent = TraceContextPropagator::new().extract(&headers);
            let span = tracing::info_span!(target: "otel::tracing", "RenewShardLease");
            span.set_parent(parent.clone()).unwrap();
            let context = span.context();
            let context_span = context.span();
            let actual = context_span.span_context();
            assert!(actual.is_valid());
            assert_eq!(actual.trace_id(), parent.span().span_context().trace_id());
            assert_ne!(actual.span_id(), parent.span().span_context().span_id());
            assert!(!actual.is_sampled());
            assert!(!context_span.is_recording());
            let mut outgoing = std::collections::HashMap::new();
            opentelemetry::global::get_text_map_propagator(|propagator| {
                propagator.inject_context(&context, &mut outgoing);
            });
            assert!(outgoing["traceparent"].starts_with("00-0123456789abcdef0123456789abcdef-"));
            assert!(outgoing["traceparent"].ends_with("-00"));
        }
        let root = tracing::info_span!(target: "otel::tracing", "root");
        root.set_parent(opentelemetry::Context::new()).unwrap();
        assert!(root.context().span().span_context().is_valid());
        tracing::debug_span!("background").in_scope(|| {
            tracing::debug!(payload = ?DiscardedPayload, "discarded log event");
        });
    });
}

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

/// Tests for the OTLP layer's filter.
mod otlp_spec {
    use test_r::test;
    use tracing_subscriber::EnvFilter;

    use crate::tracing::directive::otlp_spec;
    use crate::tracing::target;
    use crate::tracing::test::otel::{exported_spans_filtered, named};

    fn exported(user: Option<&str>) -> Vec<String> {
        let filter = EnvFilter::builder().parse_lossy(otlp_spec(user));
        let spans = exported_spans_filtered(Box::new(filter), || {
            tracing::info_span!("golem_operation").in_scope(|| {
                tracing::info!(target: target::PLUGIN_LOG, "plugin said something");
                tracing::debug!(target: target::AGENT_RDBMS, "select 1");
                tracing::info!(target: "hyper::client", "third-party at info");
            });
        });
        spans.iter().map(|s| s.name.to_string()).collect()
    }

    fn events_on_operation(user: Option<&str>) -> usize {
        let filter = EnvFilter::builder().parse_lossy(otlp_spec(user));
        let spans = exported_spans_filtered(Box::new(filter), || {
            tracing::info_span!("golem_operation").in_scope(|| {
                tracing::info!(target: target::PLUGIN_LOG, "plugin said something");
                tracing::debug!(target: target::AGENT_RDBMS, "select 1");
                tracing::info!(target: "hyper::client", "third-party at info");
            });
        });
        named(&spans, "golem_operation").events.len()
    }

    /// Exporting already needs an endpoint configured; this is the remaining
    /// switch, and until it is set nothing is produced.
    #[test]
    fn unset_exports_nothing() {
        assert_eq!(otlp_spec(None), "off");
        assert!(exported(None).is_empty());
    }

    #[test]
    fn blank_is_treated_as_unset() {
        assert_eq!(otlp_spec(Some("   ")), "off");
    }

    /// A set-but-empty `POD_NAME` - a downward-API field or Helm template that
    /// rendered blank - must fall through to the hostname rather than blanking
    /// the attribute for every replica.
    #[test]
    fn a_blank_pod_name_falls_through_to_the_hostname() {
        use crate::tracing::instance_id_from;

        assert_eq!(
            instance_id_from(Some("  ".to_string()), Some("node-7".to_string())),
            Some("node-7".to_string())
        );
        assert_eq!(
            instance_id_from(Some("pod-3".to_string()), Some("node-7".to_string())),
            Some("pod-3".to_string())
        );
        assert_eq!(instance_id_from(Some(String::new()), None), None);
        assert_eq!(instance_id_from(None, None), None);
    }

    /// A variable set to nothing must reach callers as absent, not as a filter
    /// that happens to mean `off` - otherwise `otlp_env_or`'s fallback is
    /// skipped and a caller that asked for OTLP exports nothing.
    #[test]
    fn a_blank_variable_does_not_count_as_set() {
        use crate::tracing::set_to_something;

        assert_eq!(set_to_something(String::new()), None);
        assert_eq!(set_to_something("   ".to_string()), None);
        assert_eq!(
            set_to_something("info".to_string()),
            Some("info".to_string())
        );
    }

    /// `info` means info, with nothing quietly merged in - including for
    /// third-party crates, whose noise is the deployment's judgement to make.
    #[test]
    fn info_means_info_everywhere() {
        assert_eq!(otlp_spec(Some("info")), "info");
        assert_eq!(exported(Some("info")), vec!["golem_operation"]);
        assert_eq!(
            events_on_operation(Some("info")),
            2,
            "plugin log and the third-party event, but not the debug SQL"
        );
    }

    /// The shape a deployment actually writes, quietening what it knows to be
    /// noisy for that service.
    #[test]
    fn a_deployment_can_quieten_what_it_chooses() {
        assert_eq!(
            events_on_operation(Some("info,hyper=warn,golem::plugin_log=off")),
            0
        );
    }

    /// Each source can be turned up on its own without raising anything else.
    #[test]
    fn a_label_can_be_raised_on_its_own() {
        assert_eq!(
            events_on_operation(Some("info,golem::agent_rdbms=debug")),
            3,
            "the debug SQL joins the two info events"
        );
    }

    /// Quietening the named event sources leaves the operation spans they were
    /// recorded on, since those are selected by their own module and level.
    #[test]
    fn quietening_an_event_source_keeps_the_spans() {
        assert_eq!(
            exported(Some("info,golem::plugin_log=off,golem::agent_rdbms=off")),
            vec!["golem_operation"]
        );
    }

    #[test]
    fn an_unparseable_directive_is_ignored_rather_than_fatal() {
        let filter = EnvFilter::builder().parse_lossy(otlp_spec(Some("info,not a level")));
        let spans = exported_spans_filtered(Box::new(filter), || {
            tracing::info_span!("golem_operation").in_scope(|| {});
        });
        named(&spans, "golem_operation");
    }
}

/// Tests for which filter each output gets.
///
/// The dispatch has regressed more than once: a constructor that ignored its
/// `Output` silently handed the OTLP layer the `RUST_LOG` filter, so
/// `GOLEM_OTLP_FILTER` did nothing and the startup diagnostic reported a filter
/// that was not in force.
mod output_dispatch {
    use test_r::test;
    use tracing_subscriber::filter::LevelFilter;

    use crate::tracing::Output;
    use crate::tracing::filter::for_all_outputs;

    fn max_level(filter: crate::tracing::filter::Boxed) -> Option<LevelFilter> {
        use tracing_subscriber::layer::Filter;
        Filter::<tracing_subscriber::Registry>::max_level_hint(&filter)
    }

    /// Compared against `default_otlp_env` rather than a fixed level, so this
    /// holds whatever `GOLEM_OTLP_FILTER` happens to be in the running
    /// environment - the perf skill tells developers to export it.
    ///
    /// A constructor that ignored its `Output` would hand the OTLP layer the
    /// `RUST_LOG` filter instead, and fail here. Only in the coincidence where
    /// both filters resolve to the same level would that pass unnoticed, which
    /// is why this asserts identity with the OTLP filter rather than difference
    /// from the other one - the latter false-fails on the same coincidence.
    fn otlp_gets_its_own_filter(make: impl Fn(Output) -> crate::tracing::filter::Boxed) {
        assert_eq!(
            max_level(make(Output::Otlp)),
            max_level(crate::tracing::filter::boxed::default_otlp_env()),
            "the OTLP layer must take the OTLP filter"
        );
    }

    #[test]
    fn default_env_routes_otlp_separately() {
        otlp_gets_its_own_filter(for_all_outputs::DEFAULT_ENV);
    }

    #[test]
    fn default_debug_env_routes_otlp_separately() {
        otlp_gets_its_own_filter(for_all_outputs::default_debug_env());
    }

    #[test]
    fn default_info_env_routes_otlp_separately() {
        otlp_gets_its_own_filter(for_all_outputs::default_info_env());
    }

    #[test]
    fn env_with_directives_routes_otlp_separately() {
        otlp_gets_its_own_filter(for_all_outputs::debug_env_with_directives(vec![]));
        otlp_gets_its_own_filter(for_all_outputs::info_env_with_directives(vec![]));
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
