use golem_worker_executor::worker::Worker;
use golem_worker_executor::workerctx::WorkerCtx;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use test_r::test;
use tracing::field::{Field, Visit};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

#[derive(Default)]
struct Observation {
    acquisitions: usize,
    unload_succeeded: bool,
}

#[derive(Clone, Default)]
struct Collector(Arc<Mutex<HashMap<(String, u64), Observation>>>);

fn collector() -> &'static Collector {
    static COLLECTOR: OnceLock<Collector> = OnceLock::new();
    COLLECTOR.get_or_init(Collector::default)
}

pub fn layer() -> Box<dyn Layer<Registry> + Send + Sync> {
    collector()
        .clone()
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.is_event()
                && metadata
                    .target()
                    .starts_with("golem_worker_executor::worker")
                && metadata.fields().field("execution_event").is_some()
        }))
        .boxed()
}

/// Observes ordinary runtime diagnostics without adding fields or branches to Worker.
pub trait WorkerObservations {
    fn permit_acquisitions_for_test(&self) -> usize;
    fn unload_succeeded_for_test(&self) -> bool;
}

impl<Ctx: WorkerCtx> WorkerObservations for Worker<Ctx> {
    fn permit_acquisitions_for_test(&self) -> usize {
        collector()
            .0
            .lock()
            .unwrap()
            .get(&(
                self.get_initial_worker_metadata().fingerprint.to_string(),
                self as *const Self as u64,
            ))
            .map_or(0, |observation| observation.acquisitions)
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
}

#[derive(Default)]
struct Fields {
    fingerprint: Option<String>,
    worker_shell: Option<u64>,
    event: Option<String>,
    unload_succeeded: Option<bool>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "fingerprint" => self.fingerprint = Some(value.to_owned()),
            "execution_event" => self.event = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "worker_shell" {
            self.worker_shell = Some(value);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "unload_succeeded" {
            self.unload_succeeded = Some(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }
}

impl<S: tracing::Subscriber> Layer<S> for Collector {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let (Some(fingerprint), Some(shell)) = (fields.fingerprint, fields.worker_shell) else {
            return;
        };
        let mut observations = self.0.lock().unwrap();
        let observation = observations.entry((fingerprint, shell)).or_default();
        match fields.event.as_deref() {
            Some("metadata_resolved") => *observation = Observation::default(),
            Some("permit_acquired") => observation.acquisitions += 1,
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
    tracing::subscriber::with_default(subscriber, || {
        for shell in [1u64, 2, 2] {
            tracing::debug!(fingerprint = %"same-agent", worker_shell = shell,
                execution_event = "permit_acquired", "Worker acquired a concurrent-agent permit");
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
    });
    let observed = observed.0.lock().unwrap();
    assert_eq!(observed.len(), 3);
    assert_eq!(observed[&("same-agent".into(), 1)].acquisitions, 1);
    assert_eq!(observed[&("same-agent".into(), 2)].acquisitions, 1);
    assert!(!observed[&("same-agent".into(), 1)].unload_succeeded);
    assert!(observed[&("other-agent".into(), 1)].unload_succeeded);
}
