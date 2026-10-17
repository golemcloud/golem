use crate::Tracing;
use golem_common::model::OwnedAgentId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use test_r::test;
use tokio::sync::oneshot;
use tracing::field::{Field, Visit};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

test_r::inherit_test_dep!(Tracing);

const TARGET: &str = "golem_worker_executor::services::active_agents";
const MESSAGE: &str = "Selecting owner failure for entity fence";

struct Interest {
    token: Arc<()>,
    send: oneshot::Sender<()>,
}

#[derive(Clone, Default)]
struct Collector(Arc<Mutex<HashMap<String, Interest>>>);

fn collector() -> &'static Collector {
    static COLLECTOR: OnceLock<Collector> = OnceLock::new();
    COLLECTOR.get_or_init(Collector::default)
}

pub fn layer() -> Box<dyn Layer<Registry> + Send + Sync> {
    collector()
        .clone()
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.is_event() && metadata.target() == TARGET
        }))
        .boxed()
}

pub struct Subscription {
    collector: Collector,
    owner: String,
    token: Arc<()>,
    receive: oneshot::Receiver<()>,
}

impl Subscription {
    pub async fn entered(&mut self) -> Result<(), oneshot::error::RecvError> {
        (&mut self.receive).await
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut interests = self.collector.0.lock().unwrap();
        if interests
            .get(&self.owner)
            .is_some_and(|interest| Arc::ptr_eq(&interest.token, &self.token))
        {
            interests.remove(&self.owner);
        }
    }
}

impl Collector {
    fn subscribe(&self, owner: String) -> Subscription {
        let (send, receive) = oneshot::channel();
        let token = Arc::new(());
        let mut interests = self.0.lock().unwrap();
        assert!(!interests.contains_key(&owner), "duplicate fence interest");
        interests.insert(
            owner.clone(),
            Interest {
                token: token.clone(),
                send,
            },
        );
        Subscription {
            collector: self.clone(),
            owner,
            token,
            receive,
        }
    }
}

/// Observes the existing before-fence event, not entity drainage or physical release.
pub fn lifecycle_fence(owner: &OwnedAgentId) -> Subscription {
    collector().subscribe(owner.to_string())
}

#[derive(Default)]
struct Fields {
    owner: Option<String>,
    kind: Option<String>,
    message: Option<String>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "owner_id" => self.owner = Some(value.to_owned()),
            "failure_kind" => self.kind = Some(value.to_owned()),
            "message" => self.message = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if matches!(field.name(), "owner_id" | "failure_kind" | "message") {
            self.record_str(field, &format!("{value:?}"));
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
        if fields.kind.as_deref() != Some("lifecycle") || fields.message.as_deref() != Some(MESSAGE)
        {
            return;
        }
        if let Some(owner) = fields.owner {
            let interest = self.0.lock().unwrap().remove(&owner);
            if let Some(interest) = interest {
                let _ = interest.send.send(());
            }
        }
    }
}

fn emit(owner: &str, kind: &str) {
    tracing::debug!(target: TARGET, owner_id = %owner, failure_kind = kind, "Selecting owner failure for entity fence");
}

#[test]
fn lifecycle_fence_capture_requires_exact_target_owner_kind_and_message(_tracing: &Tracing) {
    let collector = Collector::default();
    let mut subscription = collector.subscribe("selected owner".into());
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(collector.clone()));
    tracing::dispatcher::with_default(&dispatch, || {
        tracing::debug!(target: "other", owner_id = "selected owner", failure_kind = "lifecycle", "Selecting owner failure for entity fence");
        tracing::debug!(target: TARGET, owner_id = "selected owner", failure_kind = "lifecycle", "Another entity event");
        emit("other owner", "lifecycle");
        emit("selected owner", "infrastructure");
        assert!(matches!(
            subscription.receive.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        emit("selected owner", "lifecycle");
        assert!(collector.0.lock().unwrap().is_empty());
        assert_eq!(subscription.receive.try_recv(), Ok(()));
    });
}

#[test]
#[test_r::timeout("10s")]
async fn lifecycle_fence_capture_crosses_spawn_and_keeps_no_event_history(_tracing: &Tracing) {
    let owner = uuid::Uuid::new_v4().to_string();
    let mut subscription = collector().subscribe(owner.clone());
    tokio::spawn(async move {
        emit(&owner, "lifecycle");
    })
    .await
    .unwrap();
    assert!(
        !collector()
            .0
            .lock()
            .unwrap()
            .contains_key(&subscription.owner)
    );
    subscription.entered().await.unwrap();
}

#[test]
fn lifecycle_fence_old_drop_cannot_remove_a_replacement_subscription(_tracing: &Tracing) {
    let collector = Collector::default();
    let old = collector.subscribe("selected owner".into());
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(collector.clone()));
    tracing::dispatcher::with_default(&dispatch, || emit("selected owner", "lifecycle"));
    let current = collector.subscribe("selected owner".into());
    drop(old);
    assert_eq!(collector.0.lock().unwrap().len(), 1);
    drop(current);
    assert!(collector.0.lock().unwrap().is_empty());
}
