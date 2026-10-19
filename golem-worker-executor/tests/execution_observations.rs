use crate::native_waits::WindowReceipt;
use golem_common::model::OwnedAgentId;
use golem_common::tracing::target::NATIVE_WAIT;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_worker_executor::durable_host::tool::ToolOwnerFailureMetadata;
use golem_worker_executor::services::HasActiveAgents;
use golem_worker_executor::worker::Worker;
use golem_worker_executor::workerctx::WorkerCtx;
use golem_worker_executor_test_utils::TestWorkerExecutor;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use test_r::test;
use tracing::field::{Field, Visit};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

#[derive(Default)]
struct Observation {
    metadata_resolved: bool,
    acquisitions: usize,
    permit_registrations: usize,
    permit_releases: usize,
    missing_permit_trace: bool,
    unload_succeeded: bool,
    retained_cleanups: usize,
    monthly_checks: Vec<(u64, u64)>,
    creations: Vec<(uuid::Uuid, u64)>,
    interrupt_establishments: Vec<(u64, InterruptKind)>,
    invocation_suspensions: Vec<(String, InterruptKind)>,
    horizon_registrations: usize,
    horizon_closes: usize,
    missing_horizon_trace: bool,
}

#[derive(Clone, Default)]
struct Collector(
    Arc<Mutex<HashMap<(String, u64), Observation>>>,
    Arc<Mutex<HashMap<tracing::span::Id, Fields>>>,
);

impl Collector {
    fn window_identity(&self, fingerprint: uuid::Uuid, shell: u64) -> (u64, uuid::Uuid, u64) {
        // A failed observation must not poison the collector used by other tests.
        let windows = {
            let windows = self.1.lock().unwrap();
            windows.values().cloned().collect::<Vec<_>>()
        };
        let mut matching = windows.iter().filter(|fields| {
            fields.span_name == Some("execution_window")
                && fields.worker_shell == Some(shell)
                && fields.uuid("fingerprint_high", "fingerprint_low") == fingerprint
        });
        let window = matching.next().expect("an observed live execution window");
        assert!(
            matching.next().is_none(),
            "ambiguous live execution windows"
        );
        (
            window.integers["window_id"],
            window.uuid("start_attempt_high", "start_attempt_low"),
            window.integers["resident_generation"],
        )
    }
}

fn collector() -> &'static Collector {
    static COLLECTOR: OnceLock<Collector> = OnceLock::new();
    COLLECTOR.get_or_init(Collector::default)
}

pub fn layer() -> Box<dyn Layer<Registry> + Send + Sync> {
    collector()
        .clone()
        .with_filter(crate::test_observer_filter(|metadata| {
            (metadata.is_span()
                && metadata.target() == NATIVE_WAIT
                && matches!(
                    metadata.name(),
                    "execution_window" | "concurrent_agent_permit" | "monthly_horizon"
                ))
                || (metadata.is_event()
                    && ((metadata.target() == NATIVE_WAIT)
                        || (metadata
                            .target()
                            .starts_with("golem_worker_executor::worker")
                            && metadata.fields().field("execution_event").is_some())))
        }))
        .boxed()
}

/// Observes ordinary runtime diagnostics without adding fields or branches to Worker.
pub trait ExecutorObservations {
    fn concurrent_agent_permit_is_held(
        &self,
        owner: &OwnedAgentId,
    ) -> impl Future<Output = bool> + Send;
}

impl ExecutorObservations for TestWorkerExecutor {
    async fn concurrent_agent_permit_is_held(&self, owner: &OwnedAgentId) -> bool {
        if let Some(active) = self.active_agent(owner).await {
            active.primary().concurrent_agent_permit_is_held().await
        } else if let Some(active) = self.production_active_agent(owner).await {
            active.primary().concurrent_agent_permit_is_held().await
        } else {
            false
        }
    }
}

pub trait WorkerObservations {
    /// Requires a stable window boundary: live identity and owner capacity are separate reads.
    fn current_monthly_proposal_for_test(&self) -> WindowReceipt;
    fn monthly_checks(&self) -> Vec<(u64, u64)>;
    fn observed_creation_count(&self) -> u64;
    fn interrupt_establishment_receipts(&self) -> Vec<(u64, InterruptKind)>;
    fn invocation_suspension_receipts(&self) -> Vec<(String, InterruptKind)>;
    fn monthly_horizon_was_registered(&self) -> bool;
    fn monthly_window_active_for_test(&self) -> bool;
    fn concurrent_agent_permit_is_held(&self) -> impl Future<Output = bool> + Send;
    fn permit_acquisitions_for_test(&self) -> usize;
    fn unload_succeeded_for_test(&self) -> bool;
    fn retained_cleanup_receipts_for_test(&self) -> usize;
    fn terminal_interrupt_pending_for_test(&self) -> bool;
    fn selected_owner_failure_for_test(
        &self,
    ) -> impl Future<Output = Option<ToolOwnerFailureMetadata>> + Send;
    fn await_interrupt_for_test(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>>;
    fn raw_interrupt_for_test(&self) -> tokio::sync::broadcast::Receiver<InterruptKind>;
    fn retained_cleanup_for_test(
        &self,
    ) -> impl Future<Output = Result<(), WorkerExecutorError>> + Send;
    fn join_accepted_stops_for_test(
        &self,
    ) -> impl Future<Output = Result<(), WorkerExecutorError>> + Send;
}

impl<Ctx: WorkerCtx> WorkerObservations for Worker<Ctx> {
    fn observed_creation_count(&self) -> u64 {
        let observations = collector().0.lock().unwrap();
        let observation = observations
            .get(&(
                self.get_initial_worker_metadata().fingerprint.to_string(),
                self as *const Self as u64,
            ))
            .expect("creation observation must be registered before Worker startup");
        assert!(observation.metadata_resolved);
        observation.creations.len() as u64
    }

    fn interrupt_establishment_receipts(&self) -> Vec<(u64, InterruptKind)> {
        let observations = collector().0.lock().unwrap();
        let observation = observations
            .get(&(
                self.get_initial_worker_metadata().fingerprint.to_string(),
                self as *const Self as u64,
            ))
            .expect("interrupt observation must be registered before Worker startup");
        assert!(observation.metadata_resolved);
        observation.interrupt_establishments.clone()
    }

    fn monthly_horizon_was_registered(&self) -> bool {
        collector()
            .0
            .lock()
            .unwrap()
            .get(&(
                self.get_initial_worker_metadata().fingerprint.to_string(),
                self as *const Self as u64,
            ))
            .expect("Worker metadata diagnostics were not observed")
            .horizon_registrations
            > 0
    }

    fn invocation_suspension_receipts(&self) -> Vec<(String, InterruptKind)> {
        let observations = collector().0.lock().unwrap();
        let observation = observations
            .get(&(
                self.get_initial_worker_metadata().fingerprint.to_string(),
                self as *const Self as u64,
            ))
            .expect("suspension observation must be registered before Worker startup");
        assert!(observation.metadata_resolved);
        observation.invocation_suspensions.clone()
    }

    fn monthly_window_active_for_test(&self) -> bool {
        let fingerprint = self.get_initial_worker_metadata().fingerprint;
        let spans = collector().1.lock().unwrap();
        let active = spans.values().any(|span| {
            span.span_name == Some("monthly_horizon")
                && span.worker_shell == Some(self as *const Self as u64)
                && span.uuid("fingerprint_high", "fingerprint_low") == fingerprint.0
        });
        let observations = collector().0.lock().unwrap();
        let observation = observations
            .get(&(fingerprint.to_string(), self as *const Self as u64))
            .expect("Worker metadata diagnostics were not observed");
        assert!(observation.metadata_resolved);
        assert!(
            observation.horizon_registrations > 0,
            "no monthly horizon registration was observed"
        );
        assert!(
            !observation.missing_horizon_trace,
            "monthly horizon tracing was disabled"
        );
        if !active {
            assert_eq!(
                observation.horizon_closes, observation.horizon_registrations,
                "absence of a live horizon span is not proof of deactivation"
            );
        }
        active
    }

    fn monthly_checks(&self) -> Vec<(u64, u64)> {
        collector()
            .0
            .lock()
            .unwrap()
            .get(&(
                self.get_initial_worker_metadata().fingerprint.to_string(),
                self as *const Self as u64,
            ))
            .expect("Worker metadata diagnostics were not observed")
            .monthly_checks
            .clone()
    }

    async fn concurrent_agent_permit_is_held(&self) -> bool {
        // A running owner publishes its initial registration before releasing the lifecycle lock.
        let _ = self.is_loaded().await;
        let fingerprint = self.get_initial_worker_metadata().fingerprint;
        let shell = self as *const Self as u64;
        let spans = collector().1.lock().unwrap();
        let held = spans.values().any(|fields| {
            fields.span_name == Some("concurrent_agent_permit")
                && fields.worker_shell == Some(shell)
                && fields.uuid("fingerprint_high", "fingerprint_low") == fingerprint.0
        });
        {
            let observations = collector().0.lock().unwrap();
            let observation = observations
                .get(&(fingerprint.to_string(), shell))
                .expect("Worker metadata diagnostics were not observed");
            assert!(observation.metadata_resolved);
            assert!(
                observation.permit_registrations > 0,
                "no scheduler permit lifetime registration was observed"
            );
            assert!(
                observation.permit_registrations >= observation.acquisitions,
                "a permit acquisition lacks its lifetime registration"
            );
            assert!(
                !observation.missing_permit_trace,
                "a registered permit lifetime was not traced"
            );
            if !held {
                assert_eq!(
                    observation.permit_releases, observation.permit_registrations,
                    "absence of a live span is not proof of physical permit release"
                );
            }
        }
        held
    }

    fn current_monthly_proposal_for_test(&self) -> WindowReceipt {
        let fingerprint = self.get_initial_worker_metadata().fingerprint;
        let identity = collector().window_identity(fingerprint.0, self as *const Self as u64);
        self.owner_runtime_resources()
            .with_monthly_capacity(self.agent_mode(), |capacity| WindowReceipt {
                diagnostic_window_id: identity.0,
                fingerprint,
                start_attempt: identity.1,
                resident_generation: identity.2,
                policy_revision: capacity.policy_revision,
                period: capacity.period,
                fuel_generation: capacity.fuel_generation,
                exhaustion: capacity.exhaustion.map(|value| value.reason()),
            })
    }

    async fn selected_owner_failure_for_test(&self) -> Option<ToolOwnerFailureMetadata> {
        self.active_agents()
            .try_get_active_agent(&self.get_initial_worker_metadata().owned_agent_id())
            .await?
            .execution()
            .tool_operation_metadata()
            .owner_failure
    }

    fn terminal_interrupt_pending_for_test(&self) -> bool {
        *self.terminal_interrupt().borrow()
    }

    async fn retained_cleanup_for_test(&self) -> Result<(), WorkerExecutorError> {
        self.wait_for_unload_cleanup().await
    }

    async fn join_accepted_stops_for_test(&self) -> Result<(), WorkerExecutorError> {
        self.join_interrupt_establishment().await
    }

    fn await_interrupt_for_test(&self) -> Pin<Box<dyn Future<Output = InterruptKind> + Send>> {
        self.owner_runtime_resources()
            .execution_status()
            .read()
            .unwrap()
            .create_await_interrupt_signal()
    }

    fn raw_interrupt_for_test(&self) -> tokio::sync::broadcast::Receiver<InterruptKind> {
        let resources = self.owner_runtime_resources();
        let status = resources.execution_status();
        match &*status.read().unwrap() {
            golem_worker_executor::model::ExecutionStatus::Loading {
                interrupt_signal, ..
            }
            | golem_worker_executor::model::ExecutionStatus::Running {
                interrupt_signal, ..
            } => interrupt_signal.subscribe(),
            _ => panic!("expected a Loading or Running interrupt sender"),
        }
    }

    fn permit_acquisitions_for_test(&self) -> usize {
        let observations = collector().0.lock().unwrap();
        let observation = observations
            .get(&(
                self.get_initial_worker_metadata().fingerprint.to_string(),
                self as *const Self as u64,
            ))
            .expect("permit observations must be registered before Worker startup");
        assert!(observation.metadata_resolved);
        observation.acquisitions
    }

    fn unload_succeeded_for_test(&self) -> bool {
        collector()
            .0
            .lock()
            .unwrap()
            .get(&(
                self.get_initial_worker_metadata().fingerprint.to_string(),
                self as *const Self as u64,
            ))
            .is_some_and(|observation| observation.unload_succeeded)
    }

    fn retained_cleanup_receipts_for_test(&self) -> usize {
        collector()
            .0
            .lock()
            .unwrap()
            .get(&(
                self.get_initial_worker_metadata().fingerprint.to_string(),
                self as *const Self as u64,
            ))
            .map_or(0, |observation| observation.retained_cleanups)
    }
}

#[derive(Clone, Default)]
struct Fields {
    span_name: Option<&'static str>,
    fingerprint: Option<String>,
    worker_shell: Option<u64>,
    event: Option<String>,
    unload_succeeded: Option<bool>,
    trace_enabled: Option<bool>,
    physical_release: Option<bool>,
    interrupt_kind: Option<String>,
    idempotency_key: Option<String>,
    integers: HashMap<&'static str, u64>,
}

impl Fields {
    fn uuid(&self, high: &str, low: &str) -> uuid::Uuid {
        uuid::Uuid::from_u128(
            (u128::from(self.integers[high]) << 64) | u128::from(self.integers[low]),
        )
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "fingerprint" => self.fingerprint = Some(value.to_owned()),
            "execution_event" | "kind" => self.event = Some(value.to_owned()),
            "interrupt_kind" => self.interrupt_kind = Some(value.to_owned()),
            "idempotency_key" => self.idempotency_key = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.integers.insert(field.name(), value);
        if field.name() == "worker_shell" {
            self.worker_shell = Some(value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.integers.insert(field.name(), value as u64);
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "unload_succeeded" {
            self.unload_succeeded = Some(value);
        } else if field.name() == "trace_enabled" {
            self.trace_enabled = Some(value);
        } else if field.name() == "physical_release" {
            self.physical_release = Some(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }
}

impl<S: tracing::Subscriber> Layer<S> for Collector {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        _ctx: Context<'_, S>,
    ) {
        if attrs.metadata().target() == NATIVE_WAIT
            && matches!(
                attrs.metadata().name(),
                "execution_window" | "concurrent_agent_permit" | "monthly_horizon"
            )
        {
            let mut fields = Fields {
                span_name: Some(attrs.metadata().name()),
                ..Fields::default()
            };
            attrs.record(&mut fields);
            self.1.lock().unwrap().insert(id.clone(), fields);
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        _ctx: Context<'_, S>,
    ) {
        if let Some(fields) = self.1.lock().unwrap().get_mut(id) {
            values.record(fields);
        }
    }

    fn on_close(&self, id: tracing::span::Id, _ctx: Context<'_, S>) {
        let mut spans = self.1.lock().unwrap();
        if let Some(fields) = spans.remove(&id)
            && matches!(
                fields.span_name,
                Some("concurrent_agent_permit" | "monthly_horizon")
            )
        {
            let mut observations = self.0.lock().unwrap();
            let observation = observations
                .get_mut(&(
                    fields
                        .uuid("fingerprint_high", "fingerprint_low")
                        .to_string(),
                    fields.worker_shell.expect("permit owner shell"),
                ))
                .expect("lifetime span without Worker metadata");
            if fields.span_name == Some("concurrent_agent_permit") {
                assert_eq!(
                    fields.physical_release,
                    Some(true),
                    "permit lifetime closed without a physical release diagnostic"
                );
                observation.permit_releases += 1;
            } else {
                observation.horizon_closes += 1;
            }
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        if event.metadata().target() == NATIVE_WAIT
            && fields.event.as_deref() == Some("monthly_checked")
        {
            let window_id = fields.integers["window_id"];
            let shell = self.1.lock().unwrap().values().find_map(|span| {
                (span.span_name == Some("execution_window")
                    && span.integers["window_id"] == window_id)
                    .then_some(span.worker_shell)
                    .flatten()
            });
            if let Some(shell) = shell {
                self.0
                    .lock()
                    .unwrap()
                    .get_mut(&(
                        fields
                            .uuid("fingerprint_high", "fingerprint_low")
                            .to_string(),
                        shell,
                    ))
                    .expect("monthly check before Worker metadata")
                    .monthly_checks
                    .push((window_id, fields.integers["policy_revision"]));
            }
            return;
        }
        let (Some(fingerprint), Some(shell)) = (fields.fingerprint.take(), fields.worker_shell)
        else {
            return;
        };
        let mut observations = self.0.lock().unwrap();
        let observation = observations.entry((fingerprint, shell)).or_default();
        match fields.event.as_deref() {
            Some("metadata_resolved") => {
                *observation = Observation {
                    metadata_resolved: true,
                    ..Observation::default()
                }
            }
            Some("permit_acquired") => observation.acquisitions += 1,
            Some("interrupt_establishment_started") => {
                assert!(observation.metadata_resolved);
                let timestamp = || {
                    prost_types::Timestamp {
                        seconds: fields.integers["interrupt_seconds"] as i64,
                        nanos: fields.integers["interrupt_nanos"] as i32,
                    }
                    .into()
                };
                let kind = match fields.interrupt_kind.as_deref().expect("interrupt kind") {
                    "interrupt" => InterruptKind::Interrupt(timestamp()),
                    "suspend" => InterruptKind::Suspend(timestamp()),
                    "restart" => InterruptKind::Restart,
                    "jump" => InterruptKind::Jump,
                    "shard_lost" => InterruptKind::ShardLost,
                    other => panic!("unexpected interrupt kind: {other}"),
                };
                observation
                    .interrupt_establishments
                    .push((fields.integers["request_id"], kind));
            }
            Some("invocation_suspension_recorded") => {
                assert!(observation.metadata_resolved);
                let timestamp = prost_types::Timestamp {
                    seconds: fields.integers["interrupt_seconds"] as i64,
                    nanos: fields.integers["interrupt_nanos"] as i32,
                }
                .into();
                observation.invocation_suspensions.push((
                    fields.idempotency_key.expect("suspended invocation key"),
                    InterruptKind::Suspend(timestamp),
                ));
            }
            Some("resident_creation_started") => {
                assert!(
                    observation.metadata_resolved,
                    "creation observed before Worker initialization"
                );
                let generation = fields.integers["generation"];
                assert_eq!(
                    generation,
                    observation.creations.len() as u64 + 1,
                    "resident creation diagnostics contain a sequence gap"
                );
                observation.creations.push((
                    fields.uuid("start_attempt_hi", "start_attempt_lo"),
                    generation,
                ));
            }
            Some("monthly_horizon_registered") => {
                assert!(observation.metadata_resolved);
                observation.horizon_registrations += 1;
                observation.missing_horizon_trace |=
                    !fields.trace_enabled.expect("horizon tracing outcome");
            }
            Some("permit_registered") => {
                observation.permit_registrations += 1;
                observation.missing_permit_trace |=
                    !fields.trace_enabled.expect("permit tracing outcome");
            }
            Some("unload_cleanup_retained") => observation.retained_cleanups += 1,
            Some("unload_completed") => {
                observation.unload_succeeded = fields.unload_succeeded.expect("unload outcome");
            }
            _ => {}
        }
    }
}

#[test]
fn observations_separate_shells_and_keep_the_last_unload_outcome() {
    let observed = Collector::default();
    let subscriber = tracing_subscriber::registry().with(observed.clone());
    let timestamp: golem_common::model::Timestamp =
        "2026-10-18T12:34:56.123456789Z".parse().unwrap();
    let encoded: prost_types::Timestamp = timestamp.into();
    tracing::subscriber::with_default(subscriber, || {
        for shell in [1u64, 2, 2] {
            tracing::debug!(fingerprint = %"same-agent", worker_shell = shell,
                execution_event = "permit_acquired", "Worker acquired a concurrent-agent permit");
        }
        for shell in [1u64, 2, 2] {
            tracing::debug!(fingerprint = %"same-agent", worker_shell = shell,
                execution_event = "unload_cleanup_retained", "Worker retained final unload cleanup");
        }
        for succeeded in [true, false] {
            tracing::debug!(fingerprint = %"same-agent", worker_shell = 1u64,
                execution_event = "unload_completed", unload_succeeded = succeeded,
                "Worker runtime unload completed");
        }
        tracing::debug!(fingerprint = %"other-agent", worker_shell = 1u64,
            execution_event = "unload_completed", unload_succeeded = true,
            "Worker runtime unload completed");
        // A later Worker can occupy the same address for the same persisted agent.
        tracing::debug!(fingerprint = %"same-agent", worker_shell = 2u64,
            execution_event = "metadata_resolved", "Worker metadata resolved");
        tracing::debug!(fingerprint = %"same-agent", worker_shell = 2u64,
            execution_event = "permit_acquired", "Worker acquired a concurrent-agent permit");
        for (generation, attempt) in [(1u64, 7u64), (2, 9)] {
            tracing::debug!(fingerprint = %"same-agent", worker_shell = 2u64,
                execution_event = "resident_creation_started", generation,
                start_attempt_hi = 0u64, start_attempt_lo = attempt,
                "Starting resident creation");
        }
        tracing::debug!(fingerprint = %"same-agent", worker_shell = 2u64,
            execution_event = "interrupt_establishment_started", request_id = 43u64,
            interrupt_kind = "suspend", interrupt_seconds = encoded.seconds, interrupt_nanos = encoded.nanos,
            "Starting accepted interrupt establishment");
        tracing::debug!(fingerprint = %"same-agent", worker_shell = 2u64,
            execution_event = "invocation_suspension_recorded", idempotency_key = "automatic",
            interrupt_seconds = encoded.seconds, interrupt_nanos = encoded.nanos,
            "Recorded invocation suspension");
    });
    let observed = observed.0.lock().unwrap();
    assert_eq!(observed.len(), 3);
    assert_eq!(observed[&("same-agent".into(), 1)].acquisitions, 1);
    assert_eq!(observed[&("same-agent".into(), 2)].acquisitions, 1);
    assert_eq!(observed[&("same-agent".into(), 1)].retained_cleanups, 1);
    assert_eq!(observed[&("same-agent".into(), 2)].retained_cleanups, 0);
    assert_eq!(observed[&("other-agent".into(), 1)].retained_cleanups, 0);
    assert_eq!(
        observed[&("same-agent".into(), 2)].creations,
        vec![(uuid::Uuid::from_u128(7), 1), (uuid::Uuid::from_u128(9), 2)]
    );
    assert_eq!(
        observed[&("same-agent".into(), 2)].interrupt_establishments,
        vec![(43, InterruptKind::Suspend(timestamp))]
    );
    assert_eq!(
        observed[&("same-agent".into(), 2)].invocation_suspensions,
        vec![("automatic".to_owned(), InterruptKind::Suspend(timestamp))]
    );
    assert!(!observed[&("same-agent".into(), 1)].unload_succeeded);
    assert!(observed[&("other-agent".into(), 1)].unload_succeeded);
}

#[test]
fn observer_layers_keep_debug_evidence_without_enabling_trace_work() {
    let subscriber = tracing_subscriber::registry().with(crate::test_observer_layers());
    // Other tests can install scoped subscribers, so inspect this stack's own bound.
    assert_eq!(
        tracing::Subscriber::max_level_hint(&subscriber),
        Some(tracing::level_filters::LevelFilter::DEBUG),
        "test observers must not enable dependency trace argument construction"
    );
    tracing::subscriber::with_default(subscriber, || {
        assert!(tracing::enabled!(target: NATIVE_WAIT, tracing::Level::DEBUG));
    });
}

#[test]
fn window_evidence_exists_only_for_the_exact_live_span() {
    let observed = Collector::default();
    let subscriber = tracing_subscriber::registry().with(observed.clone());
    tracing::subscriber::with_default(subscriber, || {
        let first = tracing::debug_span!(target: NATIVE_WAIT, parent: None,
            "execution_window", window_id = 17u64, worker_shell = tracing::field::Empty,
            fingerprint_high = 3u64, fingerprint_low = 5u64,
            start_attempt_high = 7u64, start_attempt_low = 11u64,
            resident_generation = 13u64);
        first.record("worker_shell", 19u64);
        let second = tracing::debug_span!(target: NATIVE_WAIT, parent: None,
            "execution_window", window_id = 23u64, worker_shell = 29u64,
            fingerprint_high = 3u64, fingerprint_low = 5u64,
            start_attempt_high = 31u64, start_attempt_low = 37u64,
            resident_generation = 41u64);
        {
            let windows = observed.1.lock().unwrap();
            assert_eq!(windows.len(), 2);
            let fields = &windows[&first.id().unwrap()];
            assert_eq!(fields.worker_shell, Some(19));
            assert_eq!(fields.integers["window_id"], 17);
            assert_eq!(fields.integers["resident_generation"], 13);
            assert_eq!(
                fields.uuid("fingerprint_high", "fingerprint_low").as_u128(),
                55_340_232_221_128_654_853
            );
            assert_eq!(
                fields
                    .uuid("start_attempt_high", "start_attempt_low")
                    .as_u128(),
                129_127_208_515_966_861_323
            );
        }
        let fingerprint = uuid::Uuid::from_u128(55_340_232_221_128_654_853);
        assert_eq!(
            observed.window_identity(fingerprint, 19),
            (17, uuid::Uuid::from_u128(129_127_208_515_966_861_323), 13)
        );
        let duplicate = tracing::debug_span!(target: NATIVE_WAIT, parent: None,
            "execution_window", window_id = 43u64, worker_shell = 19u64,
            fingerprint_high = 3u64, fingerprint_low = 5u64,
            start_attempt_high = 47u64, start_attempt_low = 53u64,
            resident_generation = 59u64);
        assert!(std::panic::catch_unwind(|| observed.window_identity(fingerprint, 19)).is_err());
        assert!(!observed.1.is_poisoned());
        drop(duplicate);
        drop(first);
        assert!(std::panic::catch_unwind(|| observed.window_identity(fingerprint, 19)).is_err());
        assert!(!observed.1.is_poisoned());
        assert_eq!(
            observed.window_identity(fingerprint, 29),
            (23, uuid::Uuid::from_u128((31u128 << 64) | 37), 41)
        );
        {
            let windows = observed.1.lock().unwrap();
            assert_eq!(windows.len(), 1);
            assert_eq!(windows[&second.id().unwrap()].worker_shell, Some(29));
        }
        drop(second);
        assert!(observed.1.lock().unwrap().is_empty());
    });
}
