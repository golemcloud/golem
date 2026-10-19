use golem_common::model::agent::AgentMode;
use golem_worker_executor::services::resource_limits::{
    AtomicResourceEntry, MonthlyResourceExhaustion,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::field::{Field, Visit};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer};

#[derive(Clone, Debug)]
pub struct ResourceTransition {
    pub kind: String,
    pub revision: u64,
    pub effective_fuel: Option<u64>,
    pub fuel_generation: Option<u64>,
    pub fuel_delta: Option<i64>,
    pub memory_overshoot: Option<u128>,
}

#[derive(Clone, Default)]
struct Collector(Arc<Mutex<HashMap<u64, Vec<UnboundedSender<ResourceTransition>>>>>);

fn collector() -> &'static Collector {
    static COLLECTOR: OnceLock<Collector> = OnceLock::new();
    COLLECTOR.get_or_init(Collector::default)
}

pub fn layer() -> Box<dyn Layer<Registry> + Send + Sync> {
    collector()
        .clone()
        .with_filter(crate::test_observer_filter(|metadata| {
            metadata.is_event()
                && metadata.target() == "golem_worker_executor::services::resource_limits"
                && metadata.fields().field("resource_event").is_some()
        }))
        .boxed()
}

pub fn observe(entry: &AtomicResourceEntry) -> ResourceTransitions {
    let (tx, rx) = unbounded_channel();
    collector()
        .0
        .lock()
        .unwrap()
        .entry(entry as *const _ as u64)
        .or_default()
        .push(tx);
    ResourceTransitions {
        rx,
        pending: Vec::new(),
    }
}

pub struct ResourceTransitions {
    rx: UnboundedReceiver<ResourceTransition>,
    pending: Vec<ResourceTransition>,
}

impl ResourceTransitions {
    pub fn assert_policy_advanced_after(
        &mut self,
        revision: u64,
        generation: Option<u64>,
    ) -> anyhow::Result<()> {
        let receipts = self.take("policy_applied", revision);
        anyhow::ensure!(
            receipts
                .iter()
                .any(|receipt| receipt.fuel_generation > generation),
            "no policy application receipt advanced prepaid generation {generation:?} at revision {revision}: {receipts:?}"
        );
        Ok(())
    }

    /// Takes only receipts emitted since subscription, for the named transition and revision.
    pub fn take(&mut self, kind: &str, revision: u64) -> Vec<ResourceTransition> {
        while let Ok(receipt) = self.rx.try_recv() {
            self.pending.push(receipt);
        }
        let mut result = Vec::new();
        self.pending.retain(|receipt| {
            if receipt.kind == kind && receipt.revision == revision {
                result.push(receipt.clone());
                false
            } else {
                true
            }
        });
        result
    }
}

pub trait ResourceEntryObservations {
    fn monthly_capacity_is_exhausted_for_test(&self, mode: AgentMode) -> bool;
    fn prepaid_fuel_is_exhausted(&self, mode: AgentMode, generation: Option<u64>) -> bool;
}

impl ResourceEntryObservations for AtomicResourceEntry {
    fn monthly_capacity_is_exhausted_for_test(&self, mode: AgentMode) -> bool {
        self.with_monthly_capacity(mode, None, |capacity| capacity.exhaustion.is_some())
    }

    fn prepaid_fuel_is_exhausted(&self, mode: AgentMode, generation: Option<u64>) -> bool {
        self.with_monthly_capacity(mode, generation, |capacity| {
            capacity.exhaustion == Some(MonthlyResourceExhaustion::Compute)
        })
    }
}

#[derive(Default)]
struct Fields {
    entry: Option<u64>,
    event: Option<String>,
    revision: Option<u64>,
    fuel: Option<u64>,
    generation: Option<u64>,
    fuel_delta: Option<i64>,
    overshoot: Option<u128>,
}

impl Visit for Fields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "resource_entry" => self.entry = Some(value),
            "policy_revision" => self.revision = Some(value),
            "effective_fuel" => self.fuel = Some(value),
            "settled_fuel_generation" => self.generation = Some(value),
            _ => {}
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "fuel_delta" {
            self.fuel_delta = Some(value);
        }
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        if field.name() == "memory_overshoot_byte_nanoseconds" {
            self.overshoot = Some(value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "resource_event" {
            self.event = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl<S: tracing::Subscriber> Layer<S> for Collector {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let (Some(entry), Some(kind), Some(revision)) =
            (fields.entry, fields.event, fields.revision)
        else {
            return;
        };
        let receipt = ResourceTransition {
            kind,
            revision,
            effective_fuel: fields.fuel,
            fuel_generation: fields.generation,
            fuel_delta: fields.fuel_delta,
            memory_overshoot: fields.overshoot,
        };
        if let Some(listeners) = self.0.lock().unwrap().get_mut(&entry) {
            listeners.retain(|listener| listener.send(receipt.clone()).is_ok());
        }
    }
}
