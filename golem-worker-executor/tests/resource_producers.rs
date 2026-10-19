use golem_common::model::OwnedAgentId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use test_r::test;
use tokio::sync::oneshot;
use tracing::field::{Field, Visit};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

const TARGET: &str = "golem_worker_executor::worker::instance";
type Key = (String, String);

struct Interest {
    token: Arc<()>,
    send: oneshot::Sender<usize>,
}

#[derive(Clone, Default)]
struct Collector(Arc<Mutex<HashMap<Key, Interest>>>);

fn collector() -> &'static Collector {
    static COLLECTOR: OnceLock<Collector> = OnceLock::new();
    COLLECTOR.get_or_init(Collector::default)
}

pub fn layer() -> Box<dyn Layer<Registry> + Send + Sync> {
    collector()
        .clone()
        .with_filter(crate::test_observer_filter(|metadata| {
            metadata.is_event()
                && metadata.target() == TARGET
                && metadata.fields().field("live_usage_flushers").is_some()
        }))
        .boxed()
}

pub struct Observation {
    key: Key,
    token: Arc<()>,
    receive: oneshot::Receiver<usize>,
}

impl Observation {
    pub async fn count(self) -> anyhow::Result<usize> {
        let mut observation = self;
        Ok(
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut observation.receive)
                .await??,
        )
    }
}

impl Drop for Observation {
    fn drop(&mut self) {
        let mut interests = collector().0.lock().unwrap();
        if interests
            .get(&self.key)
            .is_some_and(|interest| Arc::ptr_eq(&interest.token, &self.token))
        {
            interests.remove(&self.key);
        }
    }
}

/// Subscribe before the registration or cleanup operation. This reports its actual
/// producer count at that boundary, not a cached estimate of current state.
pub fn observe(owner: &OwnedAgentId, stage: &'static str) -> Observation {
    let key = (owner.to_string(), stage.to_owned());
    let (send, receive) = oneshot::channel();
    let token = Arc::new(());
    let mut interests = collector().0.lock().unwrap();
    assert!(
        !interests.contains_key(&key),
        "duplicate producer observation"
    );
    interests.insert(
        key.clone(),
        Interest {
            token: token.clone(),
            send,
        },
    );
    Observation {
        key,
        token,
        receive,
    }
}

#[derive(Default)]
struct Fields {
    owner: Option<String>,
    stage: Option<String>,
    count: Option<usize>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "owner_id" => self.owner = Some(value.to_owned()),
            "stage" => self.stage = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_str(field, &format!("{value:?}"));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "live_usage_flushers" {
            self.count = usize::try_from(value).ok();
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
        let (Some(owner), Some(stage), Some(count)) = (fields.owner, fields.stage, fields.count)
        else {
            return;
        };
        let mut interests = self.0.lock().unwrap();
        if let Some(interest) = interests.remove(&(owner, stage)) {
            let _ = interest.send.send(count);
        }
    }
}

#[test]
fn cleanup_observation_requires_owner_stage_and_count() {
    let collector = Collector::default();
    let (send, mut receive) = oneshot::channel();
    collector.0.lock().unwrap().insert(
        ("owner".into(), "cleanup_completed".into()),
        Interest {
            token: Arc::new(()),
            send,
        },
    );
    let subscriber = tracing_subscriber::registry().with(collector);
    tracing::subscriber::with_default(subscriber, || {
        tracing::debug!(target: TARGET, owner_id = "other", stage = "cleanup_completed",
            live_usage_flushers = 0u64, "Owner resource usage producers");
        tracing::debug!(target: TARGET, owner_id = "owner", stage = "cleanup_failed",
            live_usage_flushers = 0u64, "Owner resource usage producers");
        tracing::debug!(target: TARGET, owner_id = "owner", stage = "cleanup_completed",
            "Owner resource usage producers");
        assert!(matches!(
            receive.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        tracing::debug!(target: TARGET, owner_id = "owner", stage = "cleanup_completed",
            live_usage_flushers = 2u64, "Owner resource usage producers");
        assert_eq!(receive.try_recv(), Ok(2));
    });
}
