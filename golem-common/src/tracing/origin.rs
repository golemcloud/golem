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

/// How work that was handed off to be run elsewhere should be related back to
/// whatever handed it off.
///
/// The choice is not a matter of taste: it follows from whether the originator is
/// still running when the work runs.
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
/// span themselves and relate back through a `TraceOrigin`. Sites that deliberately
/// omit a span point here rather than restating this.
#[derive(Clone, Debug)]
pub enum TraceOrigin {
    /// The originator waits for the work to finish, so its span encloses the work
    /// and the work belongs in the originator's trace as a child.
    ///
    /// This is the OpenTelemetry messaging conventions' single-message exception to
    /// their links-by-default rule.
    AwaitedBy(TraceParent),
    /// The originator returns before the work runs, so the work gets its own trace
    /// with a link back. Making it a child would report a span longer than its
    /// parent, and often one that started after its parent finished.
    TriggeredBy(TraceParent),
}

impl TraceOrigin {
    /// Captures the current span as an originator that will wait for the work.
    ///
    /// Call this where the waiting span is current: the span that stays open for
    /// the whole time the work runs, not an inner short-lived one. The captured
    /// span becomes the work's parent, so it has to enclose the work in time.
    pub fn awaited() -> Self {
        Self::AwaitedBy(TraceParent::capture_current())
    }

    /// Captures the current span as an originator that will not wait for the work.
    pub fn triggered() -> Self {
        Self::TriggeredBy(TraceParent::capture_current())
    }

    /// An origin with nothing to relate to, for work with no in-process
    /// originator - recovered from durable storage after a restart, for example.
    pub fn none() -> Self {
        Self::TriggeredBy(TraceParent::default())
    }

    /// Whether the originator is waiting for the work, and so encloses it.
    pub fn is_awaited(&self) -> bool {
        matches!(self, Self::AwaitedBy(_))
    }

    /// The same originator, related by link instead of by parent.
    ///
    /// Needed once the originator can no longer be assumed to still be open - a
    /// retry of work whose caller may have given up, for instance. A link stays
    /// truthful where a parent would claim containment that no longer holds.
    pub fn as_link(&self) -> Self {
        match self {
            Self::AwaitedBy(parent) | Self::TriggeredBy(parent) => {
                Self::TriggeredBy(parent.clone())
            }
        }
    }

    /// Relates `span` to this origin. `span` must have been created with
    /// `parent: None`.
    pub fn relate(&self, span: &tracing::Span) {
        match self {
            Self::AwaitedBy(parent) => parent.set_as_parent_of(span),
            Self::TriggeredBy(parent) => parent.add_as_link_to(span),
        }
    }
}

/// Creates a parentless span and relates it to a [`TraceOrigin`], yielding the
/// span.
///
/// The span starts with no parent so that `relate` is free to either give it the
/// originator as parent or leave it a linked root.
#[macro_export]
macro_rules! related_span {
    ($origin:expr, $level:expr, $name:expr) => {{
        let span = ::tracing::span!(parent: None, $level, $name);
        $origin.relate(&span);
        span
    }};
    ($origin:expr, $level:expr, $name:expr, $($fields:tt)*) => {{
        let span = ::tracing::span!(parent: None, $level, $name, $($fields)*);
        $origin.relate(&span);
        span
    }};
}

/// A trace parent captured from a [`tracing::Span`], holding only that span's
/// OpenTelemetry [`SpanContext`](opentelemetry::trace::SpanContext) rather than
/// the span itself.
///
/// Use this whenever the origin of an operation has to be remembered across an
/// asynchronous boundary - an invocation enqueued by one request and executed
/// after that request returned, for example.
///
/// A `tracing::Span` is unsuitable for that: it stays open until every clone of it
/// is dropped, with the consequences described on [`TraceOrigin`]. A captured
/// `SpanContext` is a small fixed-size value that holds nothing open.
///
/// Which of the two relationships to apply depends on whether the origin encloses
/// the work in time:
///
/// - [`set_as_parent_of`](Self::set_as_parent_of) when the origin waits for the
///   work, so the two belong in one trace and the parent really does contain the
///   child.
/// - [`add_as_link_to`](Self::add_as_link_to) when the origin has already
///   returned, so the work belongs in its own trace with a link back. The
///   OpenTelemetry specification is explicit that a parent is the wrong
///   relationship when it does not enclose the child, and names a "long running
///   asynchronous data processing operation that was initiated by one of many
///   fast incoming requests" as a case for a new linked trace.
///
/// That split follows the OpenTelemetry specification: links are the messaging
/// conventions' default for correlating a producer with a consumer, and
/// parent-child is permitted only for single-message scenarios.
#[derive(Clone, Debug, Default)]
pub struct TraceParent(Option<opentelemetry::trace::SpanContext>);

impl TraceParent {
    /// Captures the currently active span as a trace parent. Yields an empty
    /// parent when there is no active span, or when no OTLP layer is installed.
    pub fn capture_current() -> Self {
        Self::of(&tracing::Span::current())
    }

    /// Captures `span` as a trace parent. Yields an empty parent if `span` is
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

    /// Only the tests need to distinguish an empty parent; production code goes
    /// through [`TraceOrigin::relate`], which no-ops on one.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Makes `span` a child of the captured parent, joining the parent's trace.
    ///
    /// `span` must have been created with `parent: None`; a span that already has
    /// a contextual or explicit parent keeps it. Does nothing for an empty
    /// parent, leaving `span` a trace root.
    ///
    /// Only correct when the captured parent outlives `span` - otherwise the
    /// exported trace contains a child longer than its parent, which breaks
    /// critical-path analysis. Use [`add_as_link_to`](Self::add_as_link_to)
    /// instead when that is not guaranteed.
    pub(crate) fn set_as_parent_of(&self, span: &tracing::Span) {
        use opentelemetry::trace::TraceContextExt;
        use tracing_opentelemetry::OpenTelemetrySpanExt;

        if let Some(span_context) = &self.0 {
            span.set_parent(
                opentelemetry::Context::new().with_remote_span_context(span_context.clone()),
            );
        }
    }

    /// Records the captured parent as a link on `span`, for work that the parent
    /// caused but does not contain. `span` stays the root of its own trace. Does
    /// nothing for an empty parent.
    ///
    /// The link is recorded on the consuming span and points back at the
    /// originating one, which is the direction the OpenTelemetry messaging
    /// conventions prescribe.
    pub(crate) fn add_as_link_to(&self, span: &tracing::Span) {
        use tracing_opentelemetry::OpenTelemetrySpanExt;

        if let Some(span_context) = &self.0 {
            span.add_link(span_context.clone());
        }
    }
}
