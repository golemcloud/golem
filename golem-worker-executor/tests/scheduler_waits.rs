use crate::Tracing;
use golem_common::model::AgentId;
use golem_common::model::account::AccountId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use test_r::test;
use tokio::sync::oneshot;
use tracing::field::{Field, Visit};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

test_r::inherit_test_dep!(Tracing);

const TARGET: &str = "golem_worker_executor::services::active_agents::concurrent_agents_scheduler";

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
        .with_filter(crate::test_observer_filter(|metadata| {
            metadata.is_event() && metadata.target() == TARGET
        }))
        .boxed()
}

pub struct Subscription {
    collector: Collector,
    message: String,
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
            .get(&self.message)
            .is_some_and(|interest| Arc::ptr_eq(&interest.token, &self.token))
        {
            interests.remove(&self.message);
        }
    }
}

impl Collector {
    fn subscribe(&self, message: String) -> Subscription {
        let (send, receive) = oneshot::channel();
        let token = Arc::new(());
        let mut interests = self.0.lock().unwrap();
        assert!(
            !interests.contains_key(&message),
            "duplicate scheduler-wait interest"
        );
        interests.insert(
            message.clone(),
            Interest {
                token: token.clone(),
                send,
            },
        );
        Subscription {
            collector: self.clone(),
            message,
            token,
            receive,
        }
    }
}

pub fn queued(account: &AccountId, agent: &AgentId) -> Subscription {
    collector().subscribe(format!(
        "ConcurrentAgentsScheduler: {agent} queued in account {account}, waiting for permit"
    ))
}

#[derive(Default)]
struct Message(Option<String>);

impl Visit for Message {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0 = Some(value.to_owned());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for Collector {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != TARGET {
            return;
        }
        let mut message = Message::default();
        event.record(&mut message);
        if let Some(message) = message.0 {
            let interest = self.0.lock().unwrap().remove(&message);
            if let Some(interest) = interest {
                let _ = interest.send.send(());
            }
        }
    }
}

#[test]
fn scheduler_wait_matches_only_the_exact_existing_event_and_unindexes_before_delivery(
    _tracing: &Tracing,
) {
    let collector = Collector::default();
    let message = "unique scheduler queue event".to_string();
    let mut subscription = collector.subscribe(message.clone());
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(collector.clone()));
    tracing::dispatcher::with_default(&dispatch, || {
        tracing::debug!(target: "other", "{message}");
        tracing::debug!(target: TARGET, "another agent");
        assert!(matches!(
            subscription.receive.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        tracing::debug!(target: TARGET, "{message}");
        assert!(collector.0.lock().unwrap().is_empty());
        assert_eq!(subscription.receive.try_recv(), Ok(()));
    });
}

#[test]
fn scheduler_wait_drop_removes_only_its_own_subscription_and_keeps_no_history(_tracing: &Tracing) {
    let collector = Collector::default();
    let mut old = collector.subscribe("same agent".into());
    collector
        .0
        .lock()
        .unwrap()
        .remove("same agent")
        .unwrap()
        .send
        .send(())
        .unwrap();
    old.receive.try_recv().unwrap();
    let current = collector.subscribe("same agent".into());
    drop(old);
    assert_eq!(collector.0.lock().unwrap().len(), 1);
    drop(current);
    assert!(collector.0.lock().unwrap().is_empty());
}
