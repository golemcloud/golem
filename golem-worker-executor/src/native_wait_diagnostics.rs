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

//! Ordinary opt-in tracing on Golem's existing OTLP path. IDs are process-local
//! attributes, never execution authority, durable identity, billing, or replay input.

use crate::services::resource_limits::MonthlyCapacity;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentFingerprint, IdempotencyKey, OwnedAgentId};
use golem_common::tracing::TraceOrigin;
use golem_common::tracing::target::NATIVE_WAIT;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use uuid::Uuid;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("native diagnostic identity space exhausted")
}

pub(crate) fn enabled() -> bool {
    tracing::enabled!(target: NATIVE_WAIT, tracing::Level::DEBUG)
}

#[derive(Clone, Debug)]
pub(crate) struct WindowTrace {
    pub(crate) id: u64,
    pub(crate) fingerprint: AgentFingerprint,
    pub(crate) start_attempt: Uuid,
    pub(crate) resident_generation: u64,
    pub(crate) origin: TraceOrigin,
}

impl WindowTrace {
    pub(crate) fn open(
        fingerprint: AgentFingerprint,
        start_attempt: Uuid,
        resident_generation: u64,
    ) -> (Self, tracing::Span) {
        let id = next_id();
        // Never enter this permit-lifetime span across an await. Only its own bounded
        // attributes belong here; unrelated events must not accumulate until release.
        let span = tracing::debug_span!(target: NATIVE_WAIT, parent: None,
            "execution_window", window_id = id,
            agent_id = tracing::field::Empty, environment_id = tracing::field::Empty,
            fingerprint_high = (fingerprint.0.as_u128() >> 64) as u64,
            fingerprint_low = fingerprint.0.as_u128() as u64,
            start_attempt_high = (start_attempt.as_u128() >> 64) as u64,
            start_attempt_low = start_attempt.as_u128() as u64,
            resident_generation, physical_release = tracing::field::Empty,
        );
        let origin = if span.is_disabled() {
            TraceOrigin::none()
        } else {
            span.in_scope(TraceOrigin::capture_current)
        };
        (
            Self {
                id,
                fingerprint,
                start_attempt,
                resident_generation,
                origin,
            },
            span,
        )
    }

    pub(crate) fn event(
        &self,
        kind: &'static str,
        capacity: Option<MonthlyCapacity>,
        accepted: Option<bool>,
    ) {
        let span = tracing::debug_span!(target: NATIVE_WAIT, parent: None,
            "execution_window_observation", window_id = self.id, kind,
        );
        self.origin.add_as_link_to(&span);
        span.in_scope(|| tracing::debug!(target: NATIVE_WAIT,
            kind, window_id = self.id,
            fingerprint_high = (self.fingerprint.0.as_u128() >> 64) as u64,
            fingerprint_low = self.fingerprint.0.as_u128() as u64,
            start_attempt_high = (self.start_attempt.as_u128() >> 64) as u64,
            start_attempt_low = self.start_attempt.as_u128() as u64,
            resident_generation = self.resident_generation,
            policy_revision = capacity.map(|value| value.policy_revision),
            period_year = capacity.map(|value| value.period.year as i64),
            period_month = capacity.map(|value| value.period.month as u64),
            fuel_generation = capacity.and_then(|value| value.fuel_generation),
            exhaustion = capacity.and_then(|value| value.exhaustion.map(|value| value.reason())),
            accepted,
            "Native execution window"
        ));
    }
}

#[derive(Clone)]
pub(crate) struct WaitContext {
    pub(crate) window: Option<WindowTrace>,
    pub(crate) capacity: Option<MonthlyCapacity>,
    pub(crate) key: Option<IdempotencyKey>,
    pub(crate) owner: Option<OwnedAgentId>,
}

impl WaitContext {
    pub(crate) fn start(
        &self,
        operation: &'static str,
        stream_rep: Option<u32>,
        start: Option<OplogIndex>,
        pollable_reps: impl Iterator<Item = u32>,
    ) -> WaitDiagnostic {
        let mut diagnostic = WaitDiagnostic::new(
            self.window.clone(),
            self.capacity,
            self.key.clone(),
            operation,
            stream_rep,
            start,
            pollable_reps,
        );
        if let Some(owner) = &self.owner {
            diagnostic
                .span
                .record("agent_id", tracing::field::display(&owner.agent_id));
            diagnostic.span.record(
                "environment_id",
                tracing::field::display(&owner.environment_id),
            );
        }
        diagnostic.owner = self.owner.clone();
        diagnostic
    }
}

pub(crate) struct WaitDiagnostic {
    id: u64,
    window_id: Option<u64>,
    span: tracing::Span,
    origin: TraceOrigin,
    operation: &'static str,
    key: Option<IdempotencyKey>,
    owner: Option<OwnedAgentId>,
    stream_rep: Option<u32>,
    start: Option<OplogIndex>,
    resident_generation: Option<u64>,
    pending: bool,
    returned: bool,
}

impl WaitDiagnostic {
    pub(crate) fn new(
        window: Option<WindowTrace>,
        capacity: Option<MonthlyCapacity>,
        key: Option<IdempotencyKey>,
        operation: &'static str,
        stream_rep: Option<u32>,
        start: Option<OplogIndex>,
        pollable_reps: impl Iterator<Item = u32>,
    ) -> Self {
        let id = next_id();
        let window_id = window.as_ref().map(|value| value.id);
        let span = tracing::debug_span!(target: NATIVE_WAIT, parent: None,
            "native_wait", wait_id = id, window_id,
            agent_id = tracing::field::Empty, environment_id = tracing::field::Empty,
            operation, invocation_key = key.as_ref().map(|key| key.value.as_str()),
            stream_rep = stream_rep.map(u64::from), child_start = start.map(|value| value.as_u64()),
            was_pending = tracing::field::Empty, state = tracing::field::Empty,
        );
        if let Some(window) = &window {
            window.origin.add_as_link_to(&span);
        }
        let origin = if span.is_disabled() {
            TraceOrigin::none()
        } else {
            span.in_scope(TraceOrigin::capture_current)
        };
        span.in_scope(|| tracing::debug!(target: NATIVE_WAIT,
            kind = "wait_started", wait_id = id,
            window_id,
            invocation_key = key.as_ref().map(|key| key.value.as_str()),
            operation, stream_rep = stream_rep.map(u64::from),
            child_start = start.map(|value| value.as_u64()),
            fingerprint_high = window.as_ref().map(|value| (value.fingerprint.0.as_u128() >> 64) as u64),
            fingerprint_low = window.as_ref().map(|value| value.fingerprint.0.as_u128() as u64),
            start_attempt_high = window.as_ref().map(|value| (value.start_attempt.as_u128() >> 64) as u64),
            start_attempt_low = window.as_ref().map(|value| value.start_attempt.as_u128() as u64),
            resident_generation = window.as_ref().map(|value| value.resident_generation),
            policy_revision = capacity.map(|value| value.policy_revision),
            period_year = capacity.map(|value| value.period.year as i64),
            period_month = capacity.map(|value| value.period.month as u64),
            fuel_generation = capacity.and_then(|value| value.fuel_generation),
            exhaustion = capacity.and_then(|value| value.exhaustion.map(|value| value.reason())),
            "Native wait started"
        ));
        for (index, rep) in pollable_reps.enumerate() {
            span.in_scope(|| tracing::debug!(target: NATIVE_WAIT,
                kind = "pollable_binding", wait_id = id, index = index as u64, rep = u64::from(rep),
                "Native pollable binding"
            ));
        }
        Self {
            id,
            window_id,
            span,
            origin,
            operation,
            key,
            owner: None,
            stream_rep,
            start,
            resident_generation: window.as_ref().map(|value| value.resident_generation),
            pending: false,
            returned: false,
        }
    }

    fn state(&self, state: &'static str) {
        if state == "pending" {
            self.span.record("was_pending", true);
        } else {
            self.span.record("state", state);
        }
        // A short linked span can export a genuine Pending transition while the
        // operation span remains open. Export is still subject to OTLP filtering/batching.
        let transition = tracing::debug_span!(target: NATIVE_WAIT, parent: None,
            "native_wait_transition", wait_id = self.id, window_id = self.window_id, state,
            operation = self.operation,
            invocation_key = self.key.as_ref().map(|key| key.value.as_str()),
            agent_id = self.owner.as_ref().map(|owner| owner.agent_id.to_string()),
            environment_id = self.owner.as_ref().map(|owner| owner.environment_id.to_string()),
            stream_rep = self.stream_rep.map(u64::from),
            child_start = self.start.map(|value| value.as_u64()),
            resident_generation = self.resident_generation,
        );
        self.origin.add_as_link_to(&transition);
        transition.in_scope(|| {
            tracing::debug!(target: NATIVE_WAIT,
                kind = "wait_state", wait_id = self.id, state, "Native wait state"
            )
        });
    }

    pub(crate) fn observe<F: Future>(diagnostic: Option<Self>, future: F) -> NativeWait<F> {
        NativeWait { future, diagnostic }
    }
}

impl Drop for WaitDiagnostic {
    fn drop(&mut self) {
        if !self.returned {
            self.state("dropped");
        }
    }
}

pub(crate) struct NativeWait<F> {
    // Drop the actual future before reporting destruction of its diagnostic.
    future: F,
    diagnostic: Option<WaitDiagnostic>,
}

impl<F: Future> Future for NativeWait<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: once pinned, NativeWait never moves its future, including in Drop.
        let this = unsafe { self.get_unchecked_mut() };
        let result = unsafe { Pin::new_unchecked(&mut this.future) }.poll(cx);
        if let Some(diagnostic) = &mut this.diagnostic {
            if result.is_ready() {
                diagnostic.returned = true;
                diagnostic.state("returned");
            } else if !diagnostic.pending {
                diagnostic.pending = true;
                diagnostic.state("pending");
            }
        }
        result
    }
}

#[cfg(test)]
mod otel_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span_test_support::{Tracing, get_tracing_dependency as test_r_get_dep_tracing};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::{Arc, Mutex};
    use std::task::{Wake, Waker};
    use test_r::test;
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context as LayerContext, SubscriberExt};
    use tracing_subscriber::registry::LookupSpan;
    use tracing_subscriber::{Layer, Registry};

    #[derive(Clone, Default)]
    struct EventFields {
        kind: String,
        state: String,
        window: Option<u64>,
        released: bool,
    }
    impl Visit for EventFields {
        fn record_str(&mut self, field: &Field, value: &str) {
            match field.name() {
                "kind" => self.kind = value.to_owned(),
                "state" => self.state = value.to_owned(),
                _ => {}
            }
        }
        fn record_u64(&mut self, field: &Field, value: u64) {
            if field.name() == "window_id" {
                self.window = Some(value);
            }
        }
        fn record_bool(&mut self, field: &Field, value: bool) {
            if field.name() == "physical_release" {
                self.released = value;
            }
        }
        fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
    }
    #[derive(Clone)]
    struct Recorder {
        enabled: bool,
        events: Arc<Mutex<Vec<EventFields>>>,
    }
    impl<S> Layer<S> for Recorder
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    {
        fn enabled(&self, metadata: &tracing::Metadata<'_>, _ctx: LayerContext<'_, S>) -> bool {
            self.enabled && metadata.target() == NATIVE_WAIT
        }
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: LayerContext<'_, S>) {
            let mut fields = EventFields::default();
            event.record(&mut fields);
            self.events.lock().unwrap().push(fields);
        }
        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            id: &tracing::span::Id,
            ctx: LayerContext<'_, S>,
        ) {
            if attrs.metadata().name() == "execution_window" {
                let mut fields = EventFields::default();
                attrs.record(&mut fields);
                ctx.span(id).unwrap().extensions_mut().insert(fields);
            }
        }
        fn on_record(
            &self,
            id: &tracing::span::Id,
            values: &tracing::span::Record<'_>,
            ctx: LayerContext<'_, S>,
        ) {
            if let Some(span) = ctx.span(id)
                && let Some(fields) = span.extensions_mut().get_mut::<EventFields>()
            {
                values.record(fields);
            }
        }
        fn on_close(&self, id: tracing::span::Id, ctx: LayerContext<'_, S>) {
            if let Some(span) = ctx.span(&id)
                && let Some(fields) = span.extensions().get::<EventFields>()
            {
                let mut fields = fields.clone();
                fields.kind = if fields.released {
                    "window_closed"
                } else {
                    "window_abandoned"
                }
                .to_owned();
                self.events.lock().unwrap().push(fields);
            }
        }
    }
    #[derive(Default)]
    struct WakeCounter(AtomicUsize);
    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct Probe {
        ready: Arc<AtomicBool>,
        polls: Arc<AtomicUsize>,
        dropped: Arc<AtomicUsize>,
        waker: Waker,
    }
    impl Future for Probe {
        type Output = u64;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
            assert!(cx.waker().will_wake(&self.waker));
            self.polls.fetch_add(1, Ordering::SeqCst);
            if self.ready.load(Ordering::SeqCst) {
                Poll::Ready(7)
            } else {
                Poll::Pending
            }
        }
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn check_exact_polls(enabled: bool, completes: bool) -> Vec<String> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(Recorder {
            enabled,
            events: events.clone(),
        });
        tracing::subscriber::with_default(subscriber, || {
            let ready = Arc::new(AtomicBool::new(false));
            let polls = Arc::new(AtomicUsize::new(0));
            let dropped = Arc::new(AtomicUsize::new(0));
            let wakes = Arc::new(WakeCounter::default());
            let waker = Waker::from(wakes.clone());
            let mut cx = Context::from_waker(&waker);
            let diagnostic = super::enabled().then(|| {
                WaitDiagnostic::new(None, None, None, "probe", None, None, std::iter::empty())
            });
            let mut future = Box::pin(WaitDiagnostic::observe(
                diagnostic,
                Probe {
                    ready: ready.clone(),
                    polls: polls.clone(),
                    dropped: dropped.clone(),
                    waker: waker.clone(),
                },
            ));
            assert!(
                events
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|event: &EventFields| event.state.is_empty()),
                "constructing a future must not report Pending"
            );
            assert!(future.as_mut().poll(&mut cx).is_pending());
            assert!(future.as_mut().poll(&mut cx).is_pending());
            assert_eq!(polls.load(Ordering::SeqCst), 2);
            if completes {
                ready.store(true, Ordering::SeqCst);
                assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(7));
                assert_eq!(polls.load(Ordering::SeqCst), 3);
            }
            drop(future);
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
            assert_eq!(wakes.0.load(Ordering::SeqCst), 0);
        });
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| !event.state.is_empty())
            .map(|event| event.state.clone())
            .collect()
    }

    #[test]
    fn diagnostics_preserve_native_poll_waker_return_and_drop(_tracing: &Tracing) {
        assert!(check_exact_polls(false, true).is_empty());
        assert!(check_exact_polls(false, false).is_empty());
        assert_eq!(check_exact_polls(true, true), ["pending", "returned"]);
        assert_eq!(check_exact_polls(true, false), ["pending", "dropped"]);
    }

    #[test]
    fn already_ready_future_never_reports_pending(_tracing: &Tracing) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(Recorder {
            enabled: true,
            events: events.clone(),
        });
        tracing::subscriber::with_default(subscriber, || {
            let diagnostic =
                WaitDiagnostic::new(None, None, None, "ready", None, None, std::iter::empty());
            let mut future = Box::pin(WaitDiagnostic::observe(
                Some(diagnostic),
                std::future::ready(7),
            ));
            let waker = futures::task::noop_waker();
            assert_eq!(
                future.as_mut().poll(&mut Context::from_waker(&waker)),
                Poll::Ready(7)
            );
            drop(future);
            let states: Vec<_> = events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| !event.state.is_empty())
                .map(|event| event.state.clone())
                .collect();
            assert_eq!(states, ["returned"]);
        });
    }

    #[test]
    fn window_replacement_never_rebinds_an_existing_wait(_tracing: &Tracing) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(Recorder {
            enabled: true,
            events: events.clone(),
        });
        tracing::subscriber::with_default(subscriber, || {
            let fingerprint = AgentFingerprint::new();
            let (first, first_span) = WindowTrace::open(fingerprint, Uuid::new_v4(), 1);
            let first_id = first.id;
            let context = WaitContext {
                window: Some(first),
                capacity: None,
                key: None,
                owner: None,
            };
            let diagnostic = context.start("captured", None, None, std::iter::empty());
            let (second, second_span) = WindowTrace::open(fingerprint, Uuid::new_v4(), 2);
            let second_id = second.id;
            assert_ne!(first_id, second_id);
            first_span.record("physical_release", true);
            drop(first_span);
            drop(diagnostic);
            second_span.record("physical_release", true);
            drop(second_span);
            let events = events.lock().unwrap();
            assert_eq!(
                events
                    .iter()
                    .find(|event| event.kind == "wait_started")
                    .unwrap()
                    .window,
                Some(first_id)
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.kind == "window_closed")
                    .map(|event| event.window.unwrap())
                    .collect::<Vec<_>>(),
                vec![first_id, second_id]
            );
        });
    }
}
