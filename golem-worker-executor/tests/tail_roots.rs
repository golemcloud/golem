use crate::Tracing;
use golem_common::model::{IdempotencyKey, OwnedAgentId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use test_r::test;
use tokio::sync::oneshot;
use tracing::field::{Field, Visit};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

test_r::inherit_test_dep!(Tracing);

const TARGET: &str = "golem_worker_executor::worker::invocation";
const MESSAGE: &str = "Guest call root completed; settling tail work";
const SETTLED: &str = "Guest call tail-work settlement completed";
type Key = (String, String);

struct Interest {
    token: Arc<()>,
    send: oneshot::Sender<usize>,
}

#[derive(Clone)]
struct Collector(Arc<Mutex<HashMap<Key, Interest>>>, &'static str);

impl Default for Collector {
    fn default() -> Self {
        Self(Arc::default(), MESSAGE)
    }
}

fn collector() -> &'static Collector {
    static COLLECTOR: OnceLock<Collector> = OnceLock::new();
    COLLECTOR.get_or_init(Collector::default)
}

fn settled_collector() -> &'static Collector {
    static COLLECTOR: OnceLock<Collector> = OnceLock::new();
    COLLECTOR.get_or_init(|| Collector(Arc::default(), SETTLED))
}

pub fn layer() -> Box<dyn Layer<Registry> + Send + Sync> {
    collector()
        .clone()
        .and_then(settled_collector().clone())
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.is_event() && metadata.target() == TARGET
        }))
        .boxed()
}

pub struct Subscription {
    collector: Collector,
    key: Key,
    token: Arc<()>,
    receive: oneshot::Receiver<usize>,
}

impl Subscription {
    pub async fn returned(&mut self) -> Result<usize, oneshot::error::RecvError> {
        (&mut self.receive).await
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut interests = self.collector.0.lock().unwrap();
        if interests
            .get(&self.key)
            .is_some_and(|interest| Arc::ptr_eq(&interest.token, &self.token))
        {
            interests.remove(&self.key);
        }
    }
}

impl Collector {
    fn subscribe(&self, key: Key) -> Subscription {
        let (send, receive) = oneshot::channel();
        let token = Arc::new(());
        let mut interests = self.0.lock().unwrap();
        assert!(
            !interests.contains_key(&key),
            "duplicate root-return interest"
        );
        interests.insert(
            key.clone(),
            Interest {
                token: token.clone(),
                send,
            },
        );
        Subscription {
            collector: self.clone(),
            key,
            token,
            receive,
        }
    }
}

/// Observes actual Rust root completion, not guest task.return or the idle settlement predicate.
/// The count is a snapshot; retirement is observed at the later settlement boundary.
pub fn root_returned(owner: &OwnedAgentId, key: &IdempotencyKey) -> Subscription {
    collector().subscribe((owner.to_string(), key.to_string()))
}

pub fn tail_settled(owner: &OwnedAgentId, key: &IdempotencyKey) -> Subscription {
    settled_collector().subscribe((owner.to_string(), key.to_string()))
}

#[derive(Default)]
struct Fields {
    owner: Option<String>,
    key: Option<String>,
    message: Option<String>,
    primary: Option<bool>,
    active: Option<u64>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "owner_id" => self.owner = Some(value.to_owned()),
            "idempotency_key" => self.key = Some(value.to_owned()),
            "message" => self.message = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if matches!(field.name(), "owner_id" | "idempotency_key" | "message") {
            self.record_str(field, &format!("{value:?}"));
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "primary" {
            self.primary = Some(value);
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "active_tail_tasks" {
            self.active = Some(value);
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for Collector {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != TARGET {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        if fields.message.as_deref() != Some(self.1) || fields.primary != Some(true) {
            return;
        }
        if let (Some(owner), Some(key), Some(active)) = (fields.owner, fields.key, fields.active)
            && let Ok(active) = usize::try_from(active)
            && let Some(interest) = self.0.lock().unwrap().remove(&(owner, key))
        {
            let _ = interest.send.send(active);
        }
    }
}

fn emit(owner: &str, key: &str, primary: bool, active: usize) {
    tracing::debug!(target: TARGET, owner_id = %owner, idempotency_key = key, primary,
        active_tail_tasks = active, "Guest call root completed; settling tail work");
}

#[test]
fn root_capture_requires_exact_owner_key_primary_message_and_count(_tracing: &Tracing) {
    let collector = Collector::default();
    let mut subscription = collector.subscribe(("owner".into(), "key".into()));
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(collector.clone()));
    tracing::dispatcher::with_default(&dispatch, || {
        tracing::debug!(target: "other", owner_id = "owner", idempotency_key = "key", primary = true,
            active_tail_tasks = 3u64, "Guest call root completed; settling tail work");
        tracing::debug!(target: TARGET, owner_id = "owner", idempotency_key = "key", primary = true,
            active_tail_tasks = 3u64, "Another root event");
        emit("other", "key", true, 3);
        emit("owner", "other", true, 3);
        emit("owner", "key", false, 3);
        tracing::debug!(target: TARGET, owner_id = "owner", idempotency_key = "key", primary = true,
            "Guest call root completed; settling tail work");
        assert!(matches!(
            subscription.receive.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        emit("owner", "key", true, 3);
        assert_eq!(subscription.receive.try_recv(), Ok(3));
        assert!(collector.0.lock().unwrap().is_empty());
    });
}

#[test]
#[test_r::timeout("10s")]
async fn root_capture_crosses_spawn_without_retaining_event_history(_tracing: &Tracing) {
    let owner = uuid::Uuid::new_v4().to_string();
    let key = uuid::Uuid::new_v4().to_string();
    let mut subscription = collector().subscribe((owner.clone(), key.clone()));
    tokio::spawn(async move { emit(&owner, &key, true, 5) })
        .await
        .unwrap();
    assert_eq!(subscription.returned().await.unwrap(), 5);
    assert!(
        !collector()
            .0
            .lock()
            .unwrap()
            .contains_key(&subscription.key)
    );
}

#[test]
fn old_root_subscription_drop_keeps_its_replacement(_tracing: &Tracing) {
    let collector = Collector::default();
    let key = ("owner".into(), "key".into());
    let old = collector.subscribe(key.clone());
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(collector.clone()));
    tracing::dispatcher::with_default(&dispatch, || emit("owner", "key", true, 1));
    let mut replacement = collector.subscribe(key.clone());
    drop(old);
    assert!(collector.0.lock().unwrap().contains_key(&key));
    tracing::dispatcher::with_default(&dispatch, || emit("owner", "key", true, 2));
    assert_eq!(replacement.receive.try_recv(), Ok(2));
}

#[test]
fn tail_settlement_capture_ignores_root_and_checks(_tracing: &Tracing) {
    let collector = Collector(Arc::default(), SETTLED);
    let mut subscription = collector.subscribe(("owner".into(), "key".into()));
    let subscriber = tracing_subscriber::registry().with(collector);
    tracing::subscriber::with_default(subscriber, || {
        tracing::debug!(target: TARGET, owner_id = "owner", idempotency_key = "key", primary = true,
            active_tail_tasks = 0u64, "Guest call tail-work settlement check");
        tracing::debug!(target: TARGET, owner_id = "owner", idempotency_key = "key", primary = true,
            active_tail_tasks = 2u64, "Guest call root completed; settling tail work");
        assert!(matches!(
            subscription.receive.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        tracing::debug!(target: TARGET, owner_id = "owner", idempotency_key = "key", primary = true,
            active_tail_tasks = 3u64, "Guest call tail-work settlement completed");
        assert_eq!(subscription.receive.try_recv(), Ok(3));
    });
}
