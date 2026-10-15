use crate::Tracing;
use golem_common::model::account_usage::AccountUsagePeriod;
use golem_common::model::{AgentFingerprint, IdempotencyKey, OplogIndex};
use golem_common::tracing::target::NATIVE_WAIT;
use golem_worker_executor::worker::MonthlyProposalForTest;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use test_r::test;
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;
use uuid::Uuid;

test_r::inherit_test_dep!(Tracing);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitState {
    Pending,
    Returned,
    Dropped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowState {
    Open,
    PhysicallyReleased,
    Abandoned,
}

type SharedWaitState = Arc<Mutex<Option<WaitState>>>;
type SharedWindowState = Arc<Mutex<WindowState>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowReceipt {
    pub diagnostic_window_id: u64,
    pub fingerprint: AgentFingerprint,
    pub start_attempt: Uuid,
    pub resident_generation: u64,
    pub policy_revision: u64,
    pub period: AccountUsagePeriod,
    pub fuel_generation: Option<u64>,
    pub exhaustion: Option<&'static str>,
}

impl PartialEq<MonthlyProposalForTest> for WindowReceipt {
    fn eq(&self, other: &MonthlyProposalForTest) -> bool {
        self.diagnostic_window_id == other.diagnostic_window_id
            && self.fingerprint == other.fingerprint
            && self.start_attempt == other.start_attempt
            && self.resident_generation == other.resident_generation
            && self.policy_revision == other.policy_revision
            && self.period == other.period
            && self.fuel_generation == other.fuel_generation
            && self.exhaustion == other.exhaustion
    }
}

impl PartialEq<WindowReceipt> for MonthlyProposalForTest {
    fn eq(&self, other: &WindowReceipt) -> bool {
        other == self
    }
}

#[derive(Clone, Debug)]
pub struct WaitReceipt {
    pub invocation_key: Option<IdempotencyKey>,
    pub runtime: WindowReceipt,
    pub stream_rep: u32,
    pub operation: String,
    pub start_index: Option<OplogIndex>,
    pub start: OplogIndex,
    pub pollable_reps: Vec<u32>,
    state: SharedWaitState,
    window: SharedWindowState,
}

impl WaitReceipt {
    pub fn state(&self) -> WaitState {
        self.state
            .lock()
            .unwrap()
            .expect("a receipt requires an actual Pending observation")
    }

    pub fn window_closed(&self) -> bool {
        *self.window.lock().unwrap() == WindowState::PhysicallyReleased
    }
}

#[derive(Clone, Copy)]
enum InterestKind {
    Input,
    InputWithRetryDelay,
    Poll,
}

struct Interest {
    fingerprint: AgentFingerprint,
    kind: InterestKind,
    send: mpsc::UnboundedSender<WaitReceipt>,
}

impl Interest {
    fn matches(&self, receipt: &WaitReceipt) -> bool {
        self.fingerprint == receipt.runtime.fingerprint
            && match self.kind {
                InterestKind::Poll => receipt.operation == "poll",
                InterestKind::Input => {
                    receipt.operation != "poll"
                        && receipt.operation != "http_body_retry_delay_sleep"
                }
                InterestKind::InputWithRetryDelay => receipt.operation != "poll",
            }
    }
}

struct OutstandingWait {
    receipt: WaitReceipt,
    interests: Vec<u64>,
}

#[derive(Default)]
struct Ledger {
    next_interest: u64,
    interests: HashMap<u64, Interest>,
    waits: HashMap<u64, OutstandingWait>,
    bindings: HashMap<u64, BTreeMap<u64, u32>>,
    // Only spans that are still open are indexed. Receipts retain closed state themselves.
    windows: HashMap<u64, SharedWindowState>,
}

#[derive(Clone, Default)]
struct Collector(Arc<Mutex<Ledger>>);

fn collector() -> &'static Collector {
    static COLLECTOR: OnceLock<Collector> = OnceLock::new();
    COLLECTOR.get_or_init(Collector::default)
}

pub fn layer() -> Box<dyn Layer<Registry> + Send + Sync> {
    collector()
        .clone()
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.target() == NATIVE_WAIT
        }))
        .boxed()
}

pub struct WaitSubscription {
    id: u64,
    collector: Collector,
    receive: mpsc::UnboundedReceiver<WaitReceipt>,
}

impl WaitSubscription {
    pub async fn recv(&mut self) -> Option<WaitReceipt> {
        self.receive.recv().await
    }

    pub fn try_recv(&mut self) -> Result<WaitReceipt, mpsc::error::TryRecvError> {
        self.receive.try_recv()
    }
}

impl Drop for WaitSubscription {
    fn drop(&mut self) {
        let mut ledger = self.collector.0.lock().unwrap();
        ledger.interests.remove(&self.id);
        ledger.waits.retain(|_, wait| {
            wait.interests.retain(|id| *id != self.id);
            !wait.interests.is_empty()
        });
        let waits: Vec<_> = ledger.waits.keys().copied().collect();
        ledger.bindings.retain(|id, _| waits.contains(id));
    }
}

impl Collector {
    fn subscribe(&self, fingerprint: AgentFingerprint, kind: InterestKind) -> WaitSubscription {
        let (send, receive) = mpsc::unbounded_channel();
        let id = {
            let mut ledger = self.0.lock().unwrap();
            ledger.next_interest += 1;
            let id = ledger.next_interest;
            ledger.interests.insert(
                id,
                Interest {
                    fingerprint,
                    kind,
                    send,
                },
            );
            id
        };
        WaitSubscription {
            id,
            collector: self.clone(),
            receive,
        }
    }
}

pub fn input(fingerprint: AgentFingerprint) -> WaitSubscription {
    collector().subscribe(fingerprint, InterestKind::Input)
}

pub fn input_with_retry_delay(fingerprint: AgentFingerprint) -> WaitSubscription {
    collector().subscribe(fingerprint, InterestKind::InputWithRetryDelay)
}

pub fn poll(fingerprint: AgentFingerprint) -> WaitSubscription {
    collector().subscribe(fingerprint, InterestKind::Poll)
}

#[derive(Clone, Default)]
struct Fields {
    integers: HashMap<&'static str, u64>,
    signed: HashMap<&'static str, i64>,
    strings: HashMap<&'static str, String>,
    booleans: HashMap<&'static str, bool>,
}

impl Visit for Fields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.integers.insert(field.name(), value);
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.signed.insert(field.name(), value);
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.strings.insert(field.name(), value.to_owned());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.booleans.insert(field.name(), value);
    }
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl Fields {
    fn uuid(&self, high: &'static str, low: &'static str) -> Option<Uuid> {
        Some(Uuid::from_u128(
            (u128::from(*self.integers.get(high)?) << 64) | u128::from(*self.integers.get(low)?),
        ))
    }

    fn receipt(&self, state: SharedWaitState, window: SharedWindowState) -> Option<WaitReceipt> {
        let exhaustion = self
            .strings
            .get("exhaustion")
            .map(|value| match value.as_str() {
                "monthly compute exhausted" => "monthly compute exhausted",
                "monthly memory exhausted" => "monthly memory exhausted",
                "monthly durable storage exhausted" => "monthly durable storage exhausted",
                "monthly ephemeral storage exhausted" => "monthly ephemeral storage exhausted",
                other => panic!("unknown structured exhaustion: {other}"),
            });
        let start_index = self
            .integers
            .get("child_start")
            .copied()
            .map(OplogIndex::from_u64);
        Some(WaitReceipt {
            invocation_key: self
                .strings
                .get("invocation_key")
                .cloned()
                .map(IdempotencyKey::new),
            runtime: WindowReceipt {
                diagnostic_window_id: *self.integers.get("window_id")?,
                fingerprint: AgentFingerprint(self.uuid("fingerprint_high", "fingerprint_low")?),
                start_attempt: self.uuid("start_attempt_high", "start_attempt_low")?,
                resident_generation: *self.integers.get("resident_generation")?,
                policy_revision: *self.integers.get("policy_revision")?,
                period: AccountUsagePeriod {
                    year: *self.signed.get("period_year")? as i32,
                    month: *self.integers.get("period_month")? as u32,
                },
                fuel_generation: self.integers.get("fuel_generation").copied(),
                exhaustion,
            },
            stream_rep: self.integers.get("stream_rep").copied().unwrap_or(0) as u32,
            operation: self.strings.get("operation")?.clone(),
            start_index,
            start: start_index.unwrap_or(OplogIndex::NONE),
            pollable_reps: Vec::new(),
            state,
            window,
        })
    }
}

#[derive(Clone)]
struct NativeSpanObservation {
    wait_id: u64,
    state: SharedWaitState,
}

#[derive(Clone)]
struct WindowSpanObservation {
    window_id: u64,
    fields: Fields,
    state: SharedWindowState,
}

impl<S> Layer<S> for Collector
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        if attrs.metadata().target() != NATIVE_WAIT {
            return;
        }
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        match attrs.metadata().name() {
            "execution_window" => {
                let Some(window_id) = fields.integers.get("window_id").copied() else {
                    return;
                };
                let state = Arc::new(Mutex::new(WindowState::Open));
                assert!(
                    self.0
                        .lock()
                        .unwrap()
                        .windows
                        .insert(window_id, state.clone())
                        .is_none(),
                    "window identities must not be reused while open"
                );
                ctx.span(id)
                    .unwrap()
                    .extensions_mut()
                    .insert(WindowSpanObservation {
                        window_id,
                        fields,
                        state,
                    });
            }
            "native_wait" => {
                let Some(wait_id) = fields.integers.get("wait_id").copied() else {
                    return;
                };
                ctx.span(id)
                    .unwrap()
                    .extensions_mut()
                    .insert(NativeSpanObservation {
                        wait_id,
                        state: Arc::new(Mutex::new(None)),
                    });
            }
            _ => {}
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: Context<'_, S>,
    ) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let observation = {
            let mut extensions = span.extensions_mut();
            if let Some(window) = extensions.get_mut::<WindowSpanObservation>() {
                values.record(&mut window.fields);
            }
            extensions
                .get_mut::<NativeSpanObservation>()
                .map(|observation| observation.clone())
        };
        let Some(observation) = observation else {
            return;
        };
        let mut fields = Fields::default();
        values.record(&mut fields);
        let state = match fields.booleans.get("was_pending") {
            Some(pending) => {
                assert!(*pending, "was_pending records an actual Pending");
                assert!(!fields.strings.contains_key("state"));
                WaitState::Pending
            }
            None => match fields.strings.get("state").map(String::as_str) {
                Some("returned") => WaitState::Returned,
                Some("dropped") => WaitState::Dropped,
                Some(other) => panic!("unknown structured native wait terminal: {other}"),
                None => return,
            },
        };
        let prior = observation.state.lock().unwrap().replace(state);
        let mut ledger = self.0.lock().unwrap();
        if state != WaitState::Pending {
            ledger.waits.remove(&observation.wait_id);
            ledger.bindings.remove(&observation.wait_id);
            return;
        }
        assert!(prior.is_none(), "Pending must be observed exactly once");
        let bindings = ledger
            .bindings
            .get(&observation.wait_id)
            .map(|values| values.values().copied().collect())
            .unwrap_or_default();
        let Some(wait) = ledger.waits.get_mut(&observation.wait_id) else {
            return;
        };
        wait.receipt.pollable_reps = bindings;
        let receipt = wait.receipt.clone();
        let interests = wait.interests.clone();
        for id in interests {
            if let Some(interest) = ledger.interests.get(&id) {
                let _ = interest.send.send(receipt.clone());
            }
        }
    }

    fn on_close(&self, id: tracing::span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let (window, native) = {
            let extensions = span.extensions();
            (
                extensions.get::<WindowSpanObservation>().cloned(),
                extensions.get::<NativeSpanObservation>().cloned(),
            )
        };
        if let Some(window) = window {
            *window.state.lock().unwrap() =
                if window.fields.booleans.get("physical_release") == Some(&true) {
                    WindowState::PhysicallyReleased
                } else {
                    WindowState::Abandoned
                };
            self.0.lock().unwrap().windows.remove(&window.window_id);
        }
        if let Some(native) = native {
            let mut ledger = self.0.lock().unwrap();
            ledger.waits.remove(&native.wait_id);
            ledger.bindings.remove(&native.wait_id);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if event.metadata().target() != NATIVE_WAIT {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let Some(kind) = fields.strings.get("kind").map(String::as_str) else {
            return;
        };
        let Some(span) = ctx.event_span(event) else {
            return;
        };
        let observation = span.extensions().get::<NativeSpanObservation>().cloned();
        let Some(observation) = observation else {
            return;
        };
        let Some(id) = fields.integers.get("wait_id").copied() else {
            return;
        };
        assert_eq!(
            id, observation.wait_id,
            "source fields must belong to their native wait span"
        );
        let mut ledger = self.0.lock().unwrap();
        match kind {
            "wait_started" => {
                let Some(window_id) = fields.integers.get("window_id") else {
                    return;
                };
                let Some(window) = ledger.windows.get(window_id).cloned() else {
                    return;
                };
                let Some(receipt) = fields.receipt(observation.state, window) else {
                    return;
                };
                let interests: Vec<_> = ledger
                    .interests
                    .iter()
                    .filter_map(|(id, interest)| interest.matches(&receipt).then_some(*id))
                    .collect();
                if !interests.is_empty() {
                    assert!(
                        ledger
                            .waits
                            .insert(id, OutstandingWait { receipt, interests })
                            .is_none(),
                        "a native wait starts once"
                    );
                }
            }
            "pollable_binding" => {
                if let (Some(index), Some(rep)) =
                    (fields.integers.get("index"), fields.integers.get("rep"))
                    && ledger.waits.contains_key(&id)
                {
                    ledger
                        .bindings
                        .entry(id)
                        .or_default()
                        .insert(*index, *rep as u32);
                }
            }
            _ => {}
        }
    }
}

// These fixtures verify the structured subscriber contract, not native I/O.
// Actual native Pending/terminal and OTLP adapter evidence lives in library tests.
struct FixtureSource {
    id: u64,
    fingerprint: AgentFingerprint,
    attempt: Uuid,
    key: IdempotencyKey,
}

impl FixtureSource {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1 << 63);
        Self {
            id: NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed),
            fingerprint: AgentFingerprint::new(),
            attempt: Uuid::new_v4(),
            key: IdempotencyKey::fresh(),
        }
    }

    fn window(&self) -> tracing::Span {
        tracing::debug_span!(target: NATIVE_WAIT, parent: None,
            "execution_window", window_id = self.id, physical_release = tracing::field::Empty)
    }

    fn wait(&self, operation: &str, bindings: &[u32]) -> tracing::Span {
        let span = tracing::debug_span!(target: NATIVE_WAIT, parent: None,
            "native_wait", wait_id = self.id,
            was_pending = tracing::field::Empty, state = tracing::field::Empty);
        span.in_scope(|| {
            tracing::debug!(target: NATIVE_WAIT, kind = "wait_started", wait_id = self.id,
                window_id = self.id, operation, invocation_key = self.key.value.as_str(),
                stream_rep = 7u64, child_start = 11u64,
                fingerprint_high = (self.fingerprint.0.as_u128() >> 64) as u64,
                fingerprint_low = self.fingerprint.0.as_u128() as u64,
                start_attempt_high = (self.attempt.as_u128() >> 64) as u64,
                start_attempt_low = self.attempt.as_u128() as u64,
                resident_generation = 3u64, policy_revision = 5u64,
                period_year = 2026i64, period_month = 10u64, fuel_generation = 4u64,
                "Fixture field contract");
            for (index, rep) in bindings.iter().enumerate() {
                tracing::debug!(target: NATIVE_WAIT, kind = "pollable_binding", wait_id = self.id,
                    index = index as u64, rep = u64::from(*rep), "Fixture field contract");
            }
        });
        span
    }

    fn assert_identity(&self, receipt: &WaitReceipt) {
        assert_eq!(receipt.invocation_key.as_ref(), Some(&self.key));
        assert_eq!(receipt.start_index, Some(OplogIndex::from_u64(11)));
        assert_eq!(receipt.stream_rep, 7);
        assert_eq!(receipt.runtime.diagnostic_window_id, self.id);
        assert_eq!(receipt.runtime.fingerprint, self.fingerprint);
        assert_eq!(receipt.runtime.start_attempt, self.attempt);
        assert_eq!(receipt.runtime.resident_generation, 3);
        assert_eq!(receipt.runtime.policy_revision, 5);
        assert_eq!(
            receipt.runtime.period,
            AccountUsagePeriod {
                year: 2026,
                month: 10
            }
        );
        assert_eq!(receipt.runtime.fuel_generation, Some(4));
    }
}

fn index_sizes(collector: &Collector) -> (usize, usize, usize, usize) {
    let ledger = collector.0.lock().unwrap();
    (
        ledger.interests.len(),
        ledger.waits.len(),
        ledger.bindings.len(),
        ledger.windows.len(),
    )
}

fn local_dispatch(collector: &Collector) -> tracing::Dispatch {
    use tracing_subscriber::layer::SubscriberExt;
    tracing::Dispatch::new(
        tracing_subscriber::registry().with(collector.clone().with_filter(
            tracing_subscriber::filter::filter_fn(|metadata| metadata.target() == NATIVE_WAIT),
        )),
    )
}

#[test]
#[test_r::timeout("10s")]
async fn structured_collector_keeps_cross_spawn_identity_and_unregisters(_tracing: &Tracing) {
    let source = FixtureSource::new();
    let mut receive = input(source.fingerprint);
    let window = source.window();
    let native = tokio::spawn(async move {
        let native = source.wait("blocking_read", &[]);
        native.record("was_pending", true);
        (source, native)
    })
    .await
    .unwrap();
    let (source, native) = native;
    let receipt = receive.recv().await.unwrap();
    source.assert_identity(&receipt);
    assert_eq!(receipt.state(), WaitState::Pending);
    drop(receive);
    assert!(!collector().0.lock().unwrap().waits.contains_key(&source.id));
    native.record("state", "dropped");
    assert_eq!(receipt.state(), WaitState::Dropped);
    drop(native);
    window.record("physical_release", true);
    drop(window);
    assert!(receipt.window_closed());
    assert!(
        !collector()
            .0
            .lock()
            .unwrap()
            .windows
            .contains_key(&source.id)
    );
}

#[test]
fn unrelated_finished_spans_leave_no_collector_history(_tracing: &Tracing) {
    let collector = Collector::default();
    tracing::dispatcher::with_default(&local_dispatch(&collector), || {
        let baseline = index_sizes(&collector);
        for _ in 0..128 {
            let source = FixtureSource::new();
            let window = source.window();
            assert_eq!(index_sizes(&collector), (0, 0, 0, 1));
            let native = source.wait("unrelated_native_operation", &[3, 7, 3]);
            native.record("was_pending", true);
            native.record("state", "returned");
            drop(native);
            window.record("physical_release", true);
            drop(window);
            assert_eq!(index_sizes(&collector), baseline);
        }
    });
}

#[test]
fn terminal_wait_is_unindexed_before_queued_receipt_is_consumed(_tracing: &Tracing) {
    let collector = Collector::default();
    tracing::dispatcher::with_default(&local_dispatch(&collector), || {
        let source = FixtureSource::new();
        let window = source.window();
        // Registration after window creation still binds to the actual open span.
        let mut receive = collector.subscribe(source.fingerprint, InterestKind::Poll);
        let native = source.wait("poll", &[3, 7, 3]);
        native.record("was_pending", true);
        assert_eq!(index_sizes(&collector), (1, 1, 1, 1));
        native.record("state", "returned");
        assert_eq!(index_sizes(&collector), (1, 0, 0, 1));
        let receipt = receive.try_recv().unwrap();
        source.assert_identity(&receipt);
        assert_eq!(receipt.pollable_reps, [3, 7, 3]);
        assert_eq!(receipt.state(), WaitState::Returned);
        assert!(receive.try_recv().is_err());
        drop(native);
        window.record("physical_release", true);
        drop(window);
        assert!(receipt.window_closed());
        assert_eq!(index_sizes(&collector), (1, 0, 0, 0));
        drop(receive);
        assert_eq!(index_sizes(&collector), (0, 0, 0, 0));
        assert_eq!(receipt.state(), WaitState::Returned);
    });
}

#[test]
fn unregistered_receipts_keep_later_terminal_and_actual_window_close_state(_tracing: &Tracing) {
    let collector = Collector::default();
    tracing::dispatcher::with_default(&local_dispatch(&collector), || {
        for terminal in [WaitState::Returned, WaitState::Dropped] {
            for physical in [false, true] {
                let source = FixtureSource::new();
                let window = source.window();
                let mut receive = collector.subscribe(source.fingerprint, InterestKind::Input);
                let native = source.wait("unanticipated_native_stage", &[3, 7, 3]);
                native.record("was_pending", true);
                let receipt = receive.try_recv().unwrap();
                assert_eq!(
                    receipt.operation, "unanticipated_native_stage",
                    "unexpected input stages must reach callers, not be filtered out"
                );
                drop(receive);
                assert_eq!(index_sizes(&collector), (0, 0, 0, 1));
                native.record(
                    "state",
                    if terminal == WaitState::Returned {
                        "returned"
                    } else {
                        "dropped"
                    },
                );
                drop(native);
                assert_eq!(receipt.state(), terminal);
                source.assert_identity(&receipt);
                assert_eq!(receipt.pollable_reps, [3, 7, 3]);
                assert!(!receipt.window_closed());
                if physical {
                    window.record("physical_release", true);
                }
                drop(window);
                assert_eq!(index_sizes(&collector), (0, 0, 0, 0));
                assert_eq!(receipt.window_closed(), physical);
                assert_eq!(
                    *receipt.window.lock().unwrap(),
                    if physical {
                        WindowState::PhysicallyReleased
                    } else {
                        WindowState::Abandoned
                    }
                );
            }
        }
    });
}

#[test]
fn subscription_kind_filters_keep_the_explicit_retry_sleep_contract(_tracing: &Tracing) {
    let collector = Collector::default();
    tracing::dispatcher::with_default(&local_dispatch(&collector), || {
        for operation in [
            "unanticipated_native_stage",
            "http_body_retry_delay_sleep",
            "poll",
        ] {
            let source = FixtureSource::new();
            let window = source.window();
            let mut input = collector.subscribe(source.fingerprint, InterestKind::Input);
            let mut retry =
                collector.subscribe(source.fingerprint, InterestKind::InputWithRetryDelay);
            let mut poll = collector.subscribe(source.fingerprint, InterestKind::Poll);
            let native = source.wait(operation, &[]);
            native.record("was_pending", true);
            assert_eq!(
                input.try_recv().is_ok(),
                operation != "poll" && operation != "http_body_retry_delay_sleep"
            );
            assert_eq!(retry.try_recv().is_ok(), operation != "poll");
            assert_eq!(poll.try_recv().is_ok(), operation == "poll");
            native.record("state", "returned");
            drop(native);
            drop((input, retry, poll));
            window.record("physical_release", true);
            drop(window);
            assert_eq!(index_sizes(&collector), (0, 0, 0, 0));
        }
    });
}
