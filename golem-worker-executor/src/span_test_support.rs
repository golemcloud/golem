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

//! Test support for asserting on span lifetime.
//!
//! A span covers one operation and closes when that operation finishes. A span
//! wrapped around a background loop instead of around the work the loop performs
//! never closes, so it is never exported, and `tracing-opentelemetry` retains an
//! OpenTelemetry event on it for every `tracing` event recorded inside it for as
//! long as the process runs.
//!
//! [`record_spans`] installs a recording subscriber for the current thread, so a
//! test can assert that calling an operation produces exactly one span for that
//! operation and that the span is closed by the time the call returns.
//!
//! The subscriber is thread-local. Work the test `await`s directly is recorded,
//! because `test_r` drives each test future with `block_on` on a single thread;
//! work handed to `tokio::spawn` is not. That asymmetry is deliberate - it forces
//! the operation under test to be reachable directly rather than only through a
//! spawned background loop.

use std::sync::{Arc, Mutex};

use golem_common::tracing::{TracingConfig, init_tracing};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::DefaultGuard;
use tracing::{Dispatch, Event, Metadata, Subscriber, subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{Layer, Registry};

struct OpenCallsites;

impl Subscriber for OpenCallsites {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, _event: &Event<'_>) {}

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}

    fn register_callsite(
        &self,
        _metadata: &'static Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::always()
    }
}

/// Process-wide tracing state shared by tests running in one test-r worker.
pub struct Tracing {
    // A live dispatcher that is always interested prevents another thread from
    // disabling a callsite process-wide while a thread-local recorder is active.
    _open_callsites: Dispatch,
}

impl Tracing {
    pub(crate) fn init() -> Self {
        let open_callsites = Dispatch::new(OpenCallsites);
        init_tracing(
            &TracingConfig::test("worker-executor-unit-tests"),
            |_output| golem_common::tracing::filter::boxed::debug_env_with_directives(Vec::new()),
        );
        Self {
            _open_callsites: open_callsites,
        }
    }
}

pub(crate) fn get_tracing_dependency(
    dependency_view: &impl test_r::core::DependencyView,
) -> Arc<Tracing> {
    crate::test_r_get_dep_tracing(dependency_view)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedSpan {
    name: &'static str,
    /// Whether the span had closed by the time it was inspected. A span that is
    /// still open cannot have been exported.
    closed: bool,
}

#[derive(Clone, Default)]
struct RecordingLayer {
    spans: Arc<Mutex<Vec<(Id, RecordedSpan)>>>,
}

impl<S> Layer<S> for RecordingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        self.spans.lock().unwrap().push((
            id.clone(),
            RecordedSpan {
                name: attrs.metadata().name(),
                closed: false,
            },
        ));
    }

    fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
        let mut spans = self.spans.lock().unwrap();
        if let Some((_, span)) = spans.iter_mut().find(|(sid, _)| *sid == id) {
            span.closed = true;
        }
    }
}

/// Records spans created on the current thread until dropped.
pub struct SpanRecorder {
    layer: RecordingLayer,
    _guard: DefaultGuard,
}

/// Installs a span-recording subscriber for the current thread. Recording stops
/// when the returned [`SpanRecorder`] is dropped.
pub fn record_spans(_tracing: &Tracing) -> SpanRecorder {
    let layer = RecordingLayer::default();
    let guard = subscriber::set_default(Registry::default().with(layer.clone()));
    SpanRecorder {
        layer,
        _guard: guard,
    }
}

impl SpanRecorder {
    /// Spans created so far, in creation order.
    fn spans(&self) -> Vec<RecordedSpan> {
        self.layer
            .spans
            .lock()
            .unwrap()
            .iter()
            .map(|(_, span)| span.clone())
            .collect()
    }

    /// Asserts that exactly one span named `name` was created and that it closed.
    pub fn assert_closed_span(&self, name: &str) {
        let spans = self.spans();
        let matching: Vec<&RecordedSpan> = spans.iter().filter(|s| s.name == name).collect();
        let recorded: Vec<&str> = spans.iter().map(|s| s.name).collect();

        assert_eq!(
            matching.len(),
            1,
            "expected exactly one span named {name:?}, found {}; recorded: {recorded:?}",
            matching.len()
        );
        assert!(
            matching[0].closed,
            "span {name:?} was still open when the operation returned; a span that never \
             closes is never exported and retains every event recorded inside it"
        );
    }

    /// Asserts every recorded span has closed.
    pub fn assert_all_closed(&self) {
        let spans = self.spans();
        let open: Vec<&str> = spans.iter().filter(|s| !s.closed).map(|s| s.name).collect();
        assert!(open.is_empty(), "spans left open: {open:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_r_get_dep_tracing;
    use test_r::test;

    fn create_regression_span() {
        let span = tracing::info_span!("span_test_support_regression");
        let _guard = span.enter();
    }

    #[test]
    fn callsite_first_used_on_another_thread_remains_open(tracing: &Tracing) {
        let recorder = record_spans(tracing);

        std::thread::spawn(create_regression_span).join().unwrap();
        create_regression_span();

        recorder.assert_closed_span("span_test_support_regression");
    }
}
