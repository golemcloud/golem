use crate::Tracing;
use golem_common::model::{AgentId, IdempotencyKey};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use test_r::test;
use tokio::sync::oneshot;
use tracing::field::{Field, Visit};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

test_r::inherit_test_dep!(Tracing);

#[derive(Clone, Eq, Hash, PartialEq)]
struct Identity {
    agent: String,
    key: String,
}

struct Interest {
    token: Arc<()>,
    send: oneshot::Sender<()>,
}

#[derive(Clone, Default)]
struct Collector(Arc<Mutex<HashMap<Identity, Interest>>>);

fn collector() -> &'static Collector {
    static COLLECTOR: OnceLock<Collector> = OnceLock::new();
    COLLECTOR.get_or_init(Collector::default)
}

fn is_result_wait(metadata: &tracing::Metadata<'_>) -> bool {
    metadata.is_span()
        && metadata.target() == "golem_worker_executor::worker"
        && metadata.name() == "wait_for_invocation_result"
}

pub fn layer() -> Box<dyn Layer<Registry> + Send + Sync> {
    collector()
        .clone()
        .with_filter(crate::test_observer_filter(is_result_wait))
        .boxed()
}

pub struct Subscription {
    collector: Collector,
    identity: Identity,
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
            .get(&self.identity)
            .is_some_and(|interest| Arc::ptr_eq(&interest.token, &self.token))
        {
            interests.remove(&self.identity);
        }
    }
}

impl Collector {
    fn subscribe(&self, identity: Identity) -> Subscription {
        let (send, receive) = oneshot::channel();
        let token = Arc::new(());
        let mut interests = self.0.lock().unwrap();
        assert!(
            !interests.contains_key(&identity),
            "duplicate result-wait interest"
        );
        interests.insert(
            identity.clone(),
            Interest {
                token: token.clone(),
                send,
            },
        );
        Subscription {
            collector: self.clone(),
            identity,
            token,
            receive,
        }
    }
}

pub fn result_wait(agent: &AgentId, key: &IdempotencyKey) -> Subscription {
    collector().subscribe(Identity {
        agent: agent.to_string(),
        key: key.to_string(),
    })
}

#[derive(Default)]
struct IdentityFields {
    agent: Option<String>,
    key: Option<String>,
}

impl Visit for IdentityFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "agent_id" => self.agent = Some(value.to_owned()),
            "idempotency_key" => self.key = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // These existing span fields use tracing's Display visitor. No message is parsed.
        if matches!(field.name(), "agent_id" | "idempotency_key") {
            self.record_str(field, &format!("{value:?}"));
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for Collector {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: Context<'_, S>,
    ) {
        if !is_result_wait(attrs.metadata()) {
            return;
        }
        let mut fields = IdentityFields::default();
        attrs.record(&mut fields);
        let (Some(agent), Some(key)) = (fields.agent, fields.key) else {
            return;
        };
        let interest = self.0.lock().unwrap().remove(&Identity { agent, key });
        if let Some(interest) = interest {
            let _ = interest.send.send(());
        }
    }
}

fn fixture_identity() -> Identity {
    Identity {
        agent: uuid::Uuid::new_v4().to_string(),
        key: uuid::Uuid::new_v4().to_string(),
    }
}

fn result_span(identity: &Identity) -> tracing::Span {
    tracing::info_span!(target: "golem_worker_executor::worker", parent: None,
        "wait_for_invocation_result", agent_id = %identity.agent, idempotency_key = %identity.key)
}

fn local_dispatch(collector: &Collector) -> tracing::Dispatch {
    tracing::Dispatch::new(tracing_subscriber::registry().with(collector.clone()))
}

#[test]
#[test_r::timeout("10s")]
async fn result_wait_capture_crosses_spawn_and_unindexes_before_delivery(_tracing: &Tracing) {
    let identity = fixture_identity();
    let mut subscription = collector().subscribe(identity.clone());
    tokio::spawn(async move { drop(result_span(&identity)) })
        .await
        .unwrap();
    assert!(
        !collector()
            .0
            .lock()
            .unwrap()
            .contains_key(&subscription.identity)
    );
    subscription.entered().await.unwrap();
}

#[test]
fn result_wait_capture_matches_exact_fields_not_message_or_other_span(_tracing: &Tracing) {
    let collector = Collector::default();
    let identity = fixture_identity();
    let mut subscription = collector.subscribe(identity.clone());
    tracing::dispatcher::with_default(&local_dispatch(&collector), || {
        drop(result_span(&Identity {
            agent: "other".into(),
            key: identity.key.clone(),
        }));
        drop(result_span(&Identity {
            agent: identity.agent.clone(),
            key: "other".into(),
        }));
        drop(
            tracing::info_span!(target: "other", "wait_for_invocation_result",
            agent_id = %identity.agent, idempotency_key = %identity.key),
        );
        drop(
            tracing::info_span!(target: "golem_worker_executor::worker", "other",
            agent_id = %identity.agent, idempotency_key = %identity.key),
        );
        tracing::info!(target: "golem_worker_executor::worker", agent_id = %identity.agent,
            idempotency_key = %identity.key, "wait_for_invocation_result");
        assert!(matches!(
            subscription.receive.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(collector.0.lock().unwrap().len(), 1);
        drop(result_span(&identity));
        assert_eq!(subscription.receive.try_recv(), Ok(()));
        assert!(collector.0.lock().unwrap().is_empty());
    });
}

#[test]
fn result_wait_capture_has_no_history_and_old_drop_cannot_remove_replacement(_tracing: &Tracing) {
    let collector = Collector::default();
    let identity = fixture_identity();
    tracing::dispatcher::with_default(&local_dispatch(&collector), || {
        for _ in 0..128 {
            drop(result_span(&identity));
        }
        assert!(collector.0.lock().unwrap().is_empty());
        let mut old = collector.subscribe(identity.clone());
        assert!(matches!(
            old.receive.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        drop(result_span(&identity));
        assert_eq!(old.receive.try_recv(), Ok(()));
        let mut replacement = collector.subscribe(identity.clone());
        drop(old);
        assert_eq!(collector.0.lock().unwrap().len(), 1);
        drop(result_span(&identity));
        assert_eq!(replacement.receive.try_recv(), Ok(()));
        assert!(collector.0.lock().unwrap().is_empty());
        drop(collector.subscribe(identity.clone()));
        assert!(collector.0.lock().unwrap().is_empty());
        drop(result_span(&identity));
        assert!(collector.0.lock().unwrap().is_empty());
    });
}
