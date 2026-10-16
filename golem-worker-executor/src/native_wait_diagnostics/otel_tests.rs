use super::*;
use crate::durable_host::io::native_wait::NativeIoWait;
use crate::services::active_agents::ConcurrentAgentsScheduler;
use crate::services::resource_limits::AtomicResourceEntry;
use crate::span_test_support::{Tracing, get_tracing_dependency as test_r_get_dep_tracing};
use golem_common::model::account::AccountId;
use golem_common::model::component::ComponentId;
use golem_common::model::{AgentId, Timestamp};
use golem_service_base::error::worker_executor::InterruptKind;
use opentelemetry::trace::{
    Link, SpanContext, SpanId, SpanKind, TraceContextExt, TraceId, TracerProvider,
};
use opentelemetry::{Context as OtelContext, KeyValue, Value};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{
    Sampler, SamplingResult, SdkTracerProvider, ShouldSample, SimpleSpanProcessor, SpanData,
    SpanExporter,
};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use test_r::test;
use tracing::instrument::WithSubscriber;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::p2::bindings::io::streams::{HostInputStream, InputStream};
use wasmtime_wasi::p2::pipe::AsyncReadStream;

// The standard SDK adapter follows golem-common's tracing tests. This exporter
// exists only in this test module and has no transport, batching or flush step.
#[derive(Clone, Debug, Default)]
struct CollectingExporter(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for CollectingExporter {
    fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
        self.0.lock().unwrap().extend(batch);
        std::future::ready(Ok(()))
    }
}

#[derive(Clone, Debug)]
struct CountAmbientSamples(Arc<AtomicUsize>);

impl ShouldSample for CountAmbientSamples {
    fn should_sample(
        &self,
        parent_context: Option<&OtelContext>,
        trace_id: TraceId,
        name: &str,
        span_kind: &SpanKind,
        attributes: &[KeyValue],
        links: &[Link],
    ) -> SamplingResult {
        if name == "ambient_info" {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        Sampler::AlwaysOn.should_sample(
            parent_context,
            trace_id,
            name,
            span_kind,
            attributes,
            links,
        )
    }
}

fn named<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    spans
        .iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| {
            panic!(
                "missing completed span {name}; found {:?}",
                spans
                    .iter()
                    .map(|span| span.name.as_ref())
                    .collect::<Vec<&str>>()
            )
        })
}

fn attribute<'a>(span: &'a SpanData, name: &str) -> Option<&'a Value> {
    span.attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == name)
        .map(|attribute| &attribute.value)
}

fn check_disabled_origin(window: bool, context_activation: bool) {
    let exporter = CollectingExporter::default();
    let completed = exporter.0.clone();
    let samples = Arc::new(AtomicUsize::new(0));
    let provider = SdkTracerProvider::builder()
        .with_sampler(CountAmbientSamples(samples.clone()))
        .with_span_processor(SimpleSpanProcessor::new(exporter))
        .build();
    let disabled_name = if window {
        "execution_window"
    } else {
        "native_wait"
    };
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::filter::filter_fn(move |metadata| {
            !(metadata.is_span() && metadata.name() == disabled_name)
        }))
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(provider.tracer("native-wait-disabled-test"))
                .with_context_activation(context_activation),
        );
    tracing::subscriber::with_default(subscriber, || {
        let ambient = tracing::info_span!(parent: None, "ambient_info");
        let origin = ambient.in_scope(|| {
            // The SDK may sample on intentional INFO-span entry. Only work caused
            // by constructing disabled diagnostics belongs in this comparison.
            let before = samples.load(Ordering::SeqCst);
            let origin = if window {
                let (window, span) = WindowTrace::open(AgentFingerprint::new(), Uuid::new_v4(), 1);
                assert!(span.is_disabled());
                window.origin
            } else {
                let wait = WaitDiagnostic::new(
                    None,
                    None,
                    None,
                    "disabled",
                    None,
                    None,
                    std::iter::empty(),
                );
                assert!(wait.span.is_disabled());
                wait.origin.clone()
            };
            assert_eq!(
                samples.load(Ordering::SeqCst),
                before,
                "disabled diagnostics must not force ambient sampling"
            );
            origin
        });
        let probe = tracing::info_span!(parent: None, "origin_probe");
        origin.add_as_link_to(&probe);
        drop(probe);
        {
            let spans = completed.lock().unwrap().clone();
            let probe = named(&spans, "origin_probe");
            assert!(probe.span_context.is_valid());
            assert!(
                probe.links.is_empty(),
                "a disabled diagnostic must not link the unrelated ambient INFO span"
            );
            assert!(
                spans
                    .iter()
                    .filter(|span| span.name == "native_wait_transition")
                    .all(|span| span.links.is_empty())
            );
            assert!(
                !spans.iter().any(|span| span.name == "ambient_info"),
                "the ambient span remains open during the assertion"
            );
        }
        drop(ambient);
    });
}

#[test]
fn disabled_execution_window_does_not_capture_ambient_otel_origin(_tracing: &Tracing) {
    for context_activation in [false, true] {
        check_disabled_origin(true, context_activation);
    }
}

#[test]
fn disabled_native_wait_does_not_capture_ambient_otel_origin(_tracing: &Tracing) {
    for context_activation in [false, true] {
        check_disabled_origin(false, context_activation);
    }
}

fn span_context(span: &tracing::Span) -> SpanContext {
    span.context().span().span_context().clone()
}

async fn check_native_export(cancel: bool) {
    let exporter = CollectingExporter::default();
    let completed = exporter.0.clone();
    let provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .with_span_processor(SimpleSpanProcessor::new(exporter))
        .build();
    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("native-wait-adapter-test"))
            .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                metadata.target() == NATIVE_WAIT
            })),
    );
    async {
        use tokio::io::AsyncWriteExt;
        let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
        let account = AccountId(Uuid::new_v4());
        scheduler
            .register_account(
                account,
                Arc::new(AtomicResourceEntry::new(
                    u64::MAX,
                    usize::MAX,
                    usize::MAX,
                    u64::MAX,
                    1,
                )),
            )
            .await;
        let mut permit = scheduler
            .acquire(
                account,
                AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "otel-native-read".to_owned(),
                },
            )
            .await;
        let (window, window_span) = WindowTrace::open(AgentFingerprint::new(), Uuid::new_v4(), 9);
        let window_context = span_context(&window_span);
        assert!(window_context.is_valid());
        permit.observe_window(window_span);
        assert_eq!(scheduler.running_count(&account).await, Some(1));

        let (reader, mut peer) = tokio::io::duplex(8);
        let mut table = ResourceTable::new();
        let input = table
            .push(Box::new(AsyncReadStream::new(reader)) as InputStream)
            .unwrap();
        let key = IdempotencyKey::new("otel-native-read-key".to_owned());
        let diagnostic = WaitDiagnostic::new(
            Some(window.clone()),
            None,
            Some(key.clone()),
            "adapter_native_read",
            Some(input.rep()),
            None,
            std::iter::empty(),
        );
        let wait_context = span_context(&diagnostic.span);
        assert!(wait_context.is_valid());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let kind = InterruptKind::Suspend(Timestamp::now_utc());
        let native = WaitDiagnostic::observe(
            Some(diagnostic),
            HostInputStream::blocking_read(&mut table, Resource::new_borrow(input.rep()), 1),
        );
        let mut wait = Box::pin(
            NativeIoWait::with_signal(async move { stopped.await.unwrap() })
                .wait_ready_first(native),
        );
        assert!(
            futures::poll!(wait.as_mut()).is_pending(),
            "the actual Wasmtime input must be Pending before observing exports"
        );
        {
            let spans = completed.lock().unwrap().clone();
            assert!(
                !spans
                    .iter()
                    .any(|span| span.name == "native_wait" || span.name == "execution_window")
            );
            let pending = named(&spans, "native_wait_transition");
            assert_eq!(attribute(pending, "state"), Some(&Value::from("pending")));
            assert_eq!(
                attribute(pending, "operation"),
                Some(&Value::from("adapter_native_read"))
            );
            assert_eq!(
                attribute(pending, "window_id"),
                Some(&Value::from(window.id.to_string()))
            );
            assert_eq!(pending.parent_span_id, SpanId::INVALID);
            assert!(pending.span_context.is_valid());
            assert_eq!(pending.links.len(), 1);
            assert_eq!(pending.links[0].span_context, wait_context);
        }
        if cancel {
            stop.send(kind).unwrap();
            assert!(matches!(wait.as_mut().await, Err(actual) if actual == kind));
        } else {
            peer.write_all(b"x").await.unwrap();
            assert_eq!(wait.as_mut().await.unwrap().unwrap().as_slice(), b"x");
            drop(stop);
        }
        drop(wait);
        HostInputStream::drop(&mut table, Resource::new_own(input.rep()))
            .await
            .unwrap();
        {
            let spans = completed.lock().unwrap().clone();
            let native = named(&spans, "native_wait");
            assert_eq!(
                native
                    .attributes
                    .iter()
                    .filter(|attribute| attribute.key.as_str() == "state")
                    .count(),
                1,
                "the finished native span has one terminal state"
            );
            assert_eq!(
                native
                    .attributes
                    .iter()
                    .filter(|attribute| attribute.key.as_str() == "was_pending")
                    .count(),
                1
            );
            assert_eq!(attribute(native, "was_pending"), Some(&Value::from(true)));
            assert_eq!(native.span_context, wait_context);
            assert_eq!(native.parent_span_id, SpanId::INVALID);
            assert_eq!(native.links.len(), 1);
            assert_eq!(native.links[0].span_context, window_context);
            assert_eq!(
                attribute(native, "state"),
                Some(&Value::from(if cancel { "dropped" } else { "returned" }))
            );
            assert_eq!(
                attribute(native, "invocation_key"),
                Some(&Value::from(key.value))
            );
            assert_eq!(
                attribute(native, "stream_rep"),
                Some(&Value::from(input.rep().to_string()))
            );
            assert!(!spans.iter().any(|span| span.name == "execution_window"));
        }
        drop(permit);
        assert_eq!(scheduler.running_count(&account).await, Some(0));
        let spans = completed.lock().unwrap().clone();
        let released = named(&spans, "execution_window");
        assert_eq!(
            released
                .attributes
                .iter()
                .filter(|attribute| attribute.key.as_str() == "physical_release")
                .count(),
            1,
            "physical release is recorded once, never as an earlier false value"
        );
        assert_eq!(released.span_context, window_context);
        assert_eq!(
            attribute(released, "physical_release"),
            Some(&Value::from(true))
        );
        assert_eq!(
            attribute(released, "window_id"),
            Some(&Value::from(window.id.to_string()))
        );
    }
    .with_subscriber(subscriber)
    .await;
}

#[test]
#[test_r::timeout("30s")]
async fn real_native_pending_exports_short_otel_transition_before_long_spans_close(
    _tracing: &Tracing,
) {
    check_native_export(false).await;
}

#[test]
#[test_r::timeout("30s")]
async fn real_native_cancellation_exports_drop_and_physical_window_release(_tracing: &Tracing) {
    check_native_export(true).await;
}

#[test]
fn socket_binding_uses_the_existing_wait_span_and_standard_exporter(_tracing: &Tracing) {
    let exporter = CollectingExporter::default();
    let completed = exporter.0.clone();
    let provider = SdkTracerProvider::builder()
        .with_sampler(Sampler::AlwaysOn)
        .with_span_processor(SimpleSpanProcessor::new(exporter))
        .build();
    let subscriber = tracing_subscriber::registry().with(
        tracing_opentelemetry::layer().with_tracer(provider.tracer("native-socket-binding-test")),
    );
    tracing::subscriber::with_default(subscriber, || {
        let diagnostic = WaitDiagnostic::new(
            None,
            None,
            None,
            "p3_udp_receive",
            None,
            None,
            std::iter::empty(),
        );
        diagnostic.record_socket(19, "[::1]:4567".parse().unwrap());
        diagnostic.record_connection(23);
        let expected = span_context(&diagnostic.span);
        drop(diagnostic);
        let spans = completed.lock().unwrap();
        let native = named(&spans, "native_wait");
        assert_eq!(native.span_context, expected);
        assert_eq!(attribute(native, "socket_rep"), Some(&Value::from("19")));
        assert_eq!(
            attribute(native, "connection_rep"),
            Some(&Value::from("23"))
        );
        assert_eq!(
            attribute(native, "local_address"),
            Some(&Value::from("[::1]:4567"))
        );
        let event = native
            .events
            .iter()
            .find(|event| {
                event.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "kind"
                        && attribute.value == Value::from("socket_binding")
                })
            })
            .expect("socket binding is an event on the existing wait");
        for (key, value) in [
            ("wait_id", attribute(native, "wait_id").unwrap().clone()),
            ("socket_rep", Value::from("19")),
            ("local_address", Value::from("[::1]:4567")),
        ] {
            assert!(
                event
                    .attributes
                    .iter()
                    .any(|attribute| { attribute.key.as_str() == key && attribute.value == value })
            );
        }
        assert!(native.events.iter().any(|event| {
            event.attributes.iter().any(|attribute| {
                attribute.key.as_str() == "kind"
                    && attribute.value == Value::from("connection_binding")
            }) && event.attributes.iter().any(|attribute| {
                attribute.key.as_str() == "connection_rep" && attribute.value == Value::from("23")
            }) && event.attributes.iter().any(|field| {
                field.key.as_str() == "wait_id"
                    && Some(&field.value) == attribute(native, "wait_id")
            })
        }));
        assert!(
            spans.iter().all(|span| {
                span.name == "native_wait" || span.name == "native_wait_transition"
            })
        );
    });
}
