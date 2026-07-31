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

//! Relating work to whatever handed it off.
//!
//! Separate from the subscriber configuration in the parent module: this is a
//! domain model for span relationships, not tracing setup.

/// The target every [`related_span!`] span is declared under, rather than the
/// module it happens to be written in.
///
/// Filter directives silence a *module* to stop its high-volume events reaching a
/// layer, and a directive cannot distinguish a span from an event. Without a target
/// of their own, spans declared in such a module would be silenced along with its
/// events - which is backwards, since the spans are bounded and are the thing worth
/// exporting. Under this target a span is selected by its level alone.
///
/// A plain `tracing::span!` in a silenced module needs `target: SPAN_TARGET` for the
/// same reason. It also means these spans are tuned as a group: `golem::span=debug`,
/// not a per-module directive.
pub const SPAN_TARGET: &str = "golem::span";

/// Creates a parentless span and links it back to a [`TraceOrigin`], yielding the
/// span.
///
/// The span starts with no parent because it is the root of its own trace; the
/// link, not a parent, is what ties it to whatever handed the work off.
#[macro_export]
macro_rules! related_span {
    ($origin:expr, $level:expr, $name:expr) => {{
        let span = ::tracing::span!(
            target: $crate::tracing::SPAN_TARGET,
            parent: None,
            $level,
            $name
        );
        $origin.add_as_link_to(&span);
        span
    }};
    ($origin:expr, $level:expr, $name:expr, $($fields:tt)*) => {{
        let span = ::tracing::span!(
            target: $crate::tracing::SPAN_TARGET,
            parent: None,
            $level,
            $name,
            $($fields)*
        );
        $origin.add_as_link_to(&span);
        span
    }};
}

/// The span that work running elsewhere came from, holding only that span's
/// OpenTelemetry [`SpanContext`](opentelemetry::trace::SpanContext) rather than the
/// span itself.
///
/// Use this whenever the origin of an operation has to be remembered across an
/// asynchronous boundary - an invocation enqueued by one request and executed
/// after that request returned, for example.
///
/// A `tracing::Span` is unsuitable for that: it stays open until every clone of it
/// is dropped, with the consequences described under *Why background tasks get no
/// ambient span* below. A captured `SpanContext` is a small fixed-size value that
/// holds nothing open.
///
/// # The work is always linked, never nested
///
/// Work reaches this type by being handed off - enqueued, spawned, scheduled - so
/// the originator does not contain it in time. Even an invocation whose caller
/// waits for the result is handed to a worker that runs it independently: the
/// caller learns the outcome from a published event rather than from a return, and
/// the work continues if the caller goes away. Making such a span a child would
/// report a span longer than its parent, and often one that started after its
/// parent finished.
///
/// So the relationship is always [`add_as_link_to`](Self::add_as_link_to), which is
/// the OpenTelemetry messaging conventions' default for correlating a producer with
/// a consumer. The specification is explicit that a parent is the wrong
/// relationship when it does not enclose the child, and names a "long running
/// asynchronous data processing operation that was initiated by one of many fast
/// incoming requests" as a case for a new linked trace.
///
/// # Why background tasks get no ambient span
///
/// A span is only exported when it closes, and while it is open
/// `tracing-opentelemetry` appends an OpenTelemetry event to it for every `tracing`
/// event recorded inside it, without any bound. A span covering a background loop,
/// a worker's residency, or a task whose handle a guest owns therefore never
/// exports, reports a duration unrelated to any request, and retains everything
/// recorded inside it for as long as the task lives.
///
/// So such tasks are spawned with no span, and the bounded operations inside them
/// span themselves and link back through a `TraceOrigin`. Sites that deliberately
/// omit a span point here rather than restating this.
///
/// # Levels
///
/// A span must never be less likely to export than its own children, or the child
/// is exported with its parent missing. So the outermost span of a background loop
/// is `info` - it is that loop's whole operation, with nothing enclosing it - and
/// the detail inside it is `debug`.
#[derive(Clone, Debug, Default)]
pub struct TraceOrigin(Option<opentelemetry::trace::SpanContext>);

impl TraceOrigin {
    /// Captures the currently active span as the origin. Yields an empty origin
    /// when there is no active span, or when no OTLP layer is installed.
    pub fn capture_current() -> Self {
        Self::of(&tracing::Span::current())
    }

    /// Captures `span` as the origin. Yields an empty origin if `span` is
    /// disabled, or if no OTLP layer is installed.
    ///
    /// Note that this forces `span`'s sampling decision, since its span id has to
    /// be known to be recorded as a parent or link.
    fn of(span: &tracing::Span) -> Self {
        use opentelemetry::trace::TraceContextExt;
        use tracing_opentelemetry::OpenTelemetrySpanExt;

        // `context()` is not cheap: it takes the registry's extensions lock and
        // forces a sampling decision. A disabled span can never yield a valid
        // context, so skip all of it - this runs per outgoing host call.
        if span.is_disabled() {
            return Self(None);
        }

        let span_context = span.context().span().span_context().clone();
        if span_context.is_valid() {
            Self(Some(span_context))
        } else {
            Self(None)
        }
    }

    /// An origin with nothing to link to, for work with no in-process originator -
    /// recovered from durable storage after a restart, for example.
    pub fn none() -> Self {
        Self::default()
    }

    /// Only the tests need to distinguish an empty origin; production code goes
    /// through [`add_as_link_to`](Self::add_as_link_to), which no-ops on one.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Records the captured origin as a link on `span`, for work the origin caused
    /// but does not contain. `span` stays the root of its own trace. Does nothing
    /// for an empty origin.
    ///
    /// The link is recorded on the consuming span and points back at the
    /// originating one, which is the direction the OpenTelemetry messaging
    /// conventions prescribe.
    pub fn add_as_link_to(&self, span: &tracing::Span) {
        use tracing_opentelemetry::OpenTelemetrySpanExt;

        if let Some(span_context) = &self.0 {
            span.add_link(span_context.clone());
        }
    }
}
