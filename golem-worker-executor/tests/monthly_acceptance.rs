use crate::Tracing;
use crate::native_waits::WindowReceipt;
use golem_common::model::AgentFingerprint;
use golem_common::model::account_usage::AccountUsagePeriod;
use golem_common::tracing::target::NATIVE_WAIT;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, OnceLock};
use test_r::test;
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use uuid::Uuid;

test_r::inherit_test_dep!(Tracing);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Acceptance {
    pub proposal: WindowReceipt,
    pub accepted: bool,
}

struct Interest {
    token: Arc<()>,
    send: mpsc::UnboundedSender<Acceptance>,
}

#[derive(Clone, Default)]
struct Collector(Arc<Mutex<HashMap<AgentFingerprint, Interest>>>);

fn collector() -> &'static Collector {
    static COLLECTOR: OnceLock<Collector> = OnceLock::new();
    COLLECTOR.get_or_init(Collector::default)
}

pub fn layer() -> Box<dyn Layer<Registry> + Send + Sync> {
    collector()
        .clone()
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.is_event() && metadata.target() == NATIVE_WAIT
        }))
        .boxed()
}

pub struct Subscription {
    collector: Collector,
    fingerprint: AgentFingerprint,
    token: Arc<()>,
    receive: mpsc::UnboundedReceiver<Acceptance>,
}

impl Deref for Subscription {
    type Target = mpsc::UnboundedReceiver<Acceptance>;
    fn deref(&self) -> &Self::Target {
        &self.receive
    }
}

impl DerefMut for Subscription {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.receive
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut interests = self.collector.0.lock().unwrap();
        if interests
            .get(&self.fingerprint)
            .is_some_and(|interest| Arc::ptr_eq(&interest.token, &self.token))
        {
            interests.remove(&self.fingerprint);
        }
    }
}

impl Collector {
    fn subscribe(&self, fingerprint: AgentFingerprint) -> Subscription {
        let (send, receive) = mpsc::unbounded_channel();
        let token = Arc::new(());
        let mut interests = self.0.lock().unwrap();
        assert!(
            !interests.contains_key(&fingerprint),
            "duplicate monthly-acceptance interest"
        );
        interests.insert(
            fingerprint,
            Interest {
                token: token.clone(),
                send,
            },
        );
        Subscription {
            collector: self.clone(),
            fingerprint,
            token,
            receive,
        }
    }
}

/// Observes completed acceptance attempts from the ordinary window diagnostic.
pub fn observe(fingerprint: AgentFingerprint) -> Subscription {
    collector().subscribe(fingerprint)
}

#[derive(Default)]
struct Fields {
    unsigned: HashMap<&'static str, u64>,
    signed: HashMap<&'static str, i64>,
    strings: HashMap<&'static str, String>,
    accepted: Option<bool>,
}

impl Visit for Fields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.unsigned.insert(field.name(), value);
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.signed.insert(field.name(), value);
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.strings.insert(field.name(), value.to_owned());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "accepted" {
            self.accepted = Some(value);
        }
    }
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl Fields {
    fn uuid(&self, high: &'static str, low: &'static str) -> Option<Uuid> {
        Some(Uuid::from_u128(
            (u128::from(*self.unsigned.get(high)?) << 64) | u128::from(*self.unsigned.get(low)?),
        ))
    }

    fn acceptance(&self) -> Option<Acceptance> {
        if self.strings.get("kind")?.as_str() != "monthly_acceptance" {
            return None;
        }
        let exhaustion = match self.strings.get("exhaustion").map(String::as_str) {
            Some("monthly compute exhausted") => Some("monthly compute exhausted"),
            Some("monthly memory exhausted") => Some("monthly memory exhausted"),
            Some("monthly durable storage exhausted") => Some("monthly durable storage exhausted"),
            Some("monthly ephemeral storage exhausted") => {
                Some("monthly ephemeral storage exhausted")
            }
            None => None,
            Some(other) => panic!("unexpected monthly exhaustion diagnostic: {other}"),
        };
        Some(Acceptance {
            proposal: WindowReceipt {
                diagnostic_window_id: *self.unsigned.get("window_id")?,
                fingerprint: AgentFingerprint(self.uuid("fingerprint_high", "fingerprint_low")?),
                start_attempt: self.uuid("start_attempt_high", "start_attempt_low")?,
                resident_generation: *self.unsigned.get("resident_generation")?,
                policy_revision: *self.unsigned.get("policy_revision")?,
                period: AccountUsagePeriod {
                    year: (*self.signed.get("period_year")?).try_into().ok()?,
                    month: (*self.unsigned.get("period_month")?).try_into().ok()?,
                },
                fuel_generation: self.unsigned.get("fuel_generation").copied(),
                exhaustion,
            },
            accepted: self.accepted?,
        })
    }
}

impl<S: tracing::Subscriber> Layer<S> for Collector {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != NATIVE_WAIT {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let Some(acceptance) = fields.acceptance() else {
            return;
        };
        let sender = self
            .0
            .lock()
            .unwrap()
            .get(&acceptance.proposal.fingerprint)
            .map(|interest| interest.send.clone());
        if let Some(sender) = sender {
            let _ = sender.send(acceptance);
        }
    }
}

fn emit(
    fingerprint: AgentFingerprint,
    accepted: bool,
    fuel_generation: Option<u64>,
    exhaustion: Option<&str>,
) {
    tracing::debug!(target: NATIVE_WAIT, kind = "monthly_acceptance", window_id = 7_u64,
        fingerprint_high = (fingerprint.0.as_u128() >> 64) as u64,
        fingerprint_low = fingerprint.0.as_u128() as u64,
        start_attempt_high = 3_u64, start_attempt_low = 4_u64, resident_generation = 5_u64,
        policy_revision = 6_u64, period_year = 2026_i64, period_month = 10_u64,
        fuel_generation, exhaustion, accepted, "Native execution window");
}

#[test]
fn acceptance_capture_keeps_exact_fields_both_results_and_optional_fuel(_tracing: &Tracing) {
    let collector = Collector::default();
    let fingerprint = AgentFingerprint::new();
    let mut subscription = collector.subscribe(fingerprint);
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(collector.clone()));
    tracing::dispatcher::with_default(&dispatch, || {
        emit(AgentFingerprint::new(), true, None, None);
        tracing::debug!(target: NATIVE_WAIT, kind = "window_opened", accepted = true, "Native execution window");
        tracing::debug!(target: "other", kind = "monthly_acceptance", accepted = true, "Native execution window");
        assert!(matches!(
            subscription.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        for (accepted, fuel, exhaustion) in [
            (false, None, None),
            (true, Some(8), Some("monthly compute exhausted")),
        ] {
            emit(fingerprint, accepted, fuel, exhaustion);
            assert_eq!(
                subscription.try_recv().unwrap(),
                Acceptance {
                    accepted,
                    proposal: WindowReceipt {
                        diagnostic_window_id: 7,
                        fingerprint,
                        start_attempt: Uuid::from_u128((3 << 64) | 4),
                        resident_generation: 5,
                        policy_revision: 6,
                        period: AccountUsagePeriod {
                            year: 2026,
                            month: 10
                        },
                        fuel_generation: fuel,
                        exhaustion,
                    }
                }
            );
        }
        assert_eq!(collector.0.lock().unwrap().len(), 1);
    });
    drop(subscription);
    assert!(collector.0.lock().unwrap().is_empty());
}

#[test]
#[test_r::timeout("10s")]
async fn acceptance_capture_crosses_spawn_and_does_not_retain_past_events(_tracing: &Tracing) {
    let fingerprint = AgentFingerprint::new();
    emit(fingerprint, false, None, None);
    let mut subscription = observe(fingerprint);
    assert!(matches!(
        subscription.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    tokio::spawn(async move {
        emit(fingerprint, true, Some(9), Some("monthly memory exhausted"));
    })
    .await
    .unwrap();
    assert!(subscription.recv().await.unwrap().accepted);
    drop(subscription);
    assert!(!collector().0.lock().unwrap().contains_key(&fingerprint));
}
