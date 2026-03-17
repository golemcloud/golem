// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::repo::registry_change::{ChangeEventId, RegistryChangeEvent, RegistryChangeRepo};
use golem_api_grpc::proto::golem::registry::v1::{
    AccountTokensInvalidatedEvent, CursorExpiredEvent, DeploymentChangedEvent,
    DomainRegistrationChangedEvent, EnvironmentPermissionsChangedEvent,
    RegistryInvalidationEvent, SecuritySchemeChangedEvent,
    registry_invalidation_event::Payload,
};
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Manages registry change event broadcasting. Delivery is at-least-once;
/// subscribers must deduplicate by event_id.
pub trait RegistryChangeNotifier: Send + Sync {
    /// Publish an event to all subscribers.
    fn notify(&self, event: RegistryChangeEvent);

    /// Get a new broadcast receiver for subscribing to live events.
    fn subscribe(&self) -> broadcast::Receiver<RegistryChangeEvent>;

    /// Start any background tasks needed by this implementation (e.g., PgListener).
    /// No-op for implementations that don't need background tasks.
    fn start_background_tasks(
        &self,
        _join_set: &mut tokio::task::JoinSet<Result<(), anyhow::Error>>,
    ) {
        // Default no-op
    }
}

/// Local in-process notifier for single-node deployments (e.g., SQLite).
/// No background tasks needed — notify() directly publishes to the broadcast.
pub struct LocalRegistryChangeNotifier {
    sender: broadcast::Sender<RegistryChangeEvent>,
}

impl LocalRegistryChangeNotifier {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }
}

impl RegistryChangeNotifier for LocalRegistryChangeNotifier {
    fn notify(&self, event: RegistryChangeEvent) {
        let _ = self.sender.send(event);
    }

    fn subscribe(&self) -> broadcast::Receiver<RegistryChangeEvent> {
        self.sender.subscribe()
    }
}

/// Postgres-backed notifier that uses LISTEN/NOTIFY for cross-node propagation.
/// Immediate local delivery is also provided via notify() for low-latency
/// same-node event propagation; the PgListener handles cross-node delivery.
/// This means same-node subscribers may see duplicate events — dedup by event_id.
pub struct PostgresRegistryChangeNotifier {
    sender: broadcast::Sender<RegistryChangeEvent>,
    repo: Arc<dyn RegistryChangeRepo>,
    connect_url: String,
}

impl PostgresRegistryChangeNotifier {
    pub fn new(
        capacity: usize,
        repo: Arc<dyn RegistryChangeRepo>,
        pg_config: &golem_common::config::DbPostgresConfig,
    ) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        let connect_url = format!(
            "postgres://{}:{}@{}:{}/{}",
            pg_config.username,
            pg_config.password,
            pg_config.host,
            pg_config.port,
            pg_config.database
        );
        Self {
            sender,
            repo,
            connect_url,
        }
    }
}

impl RegistryChangeNotifier for PostgresRegistryChangeNotifier {
    fn notify(&self, event: RegistryChangeEvent) {
        let _ = self.sender.send(event);
    }

    fn subscribe(&self) -> broadcast::Receiver<RegistryChangeEvent> {
        self.sender.subscribe()
    }

    fn start_background_tasks(
        &self,
        join_set: &mut tokio::task::JoinSet<Result<(), anyhow::Error>>,
    ) {
        let connect_url = self.connect_url.clone();
        let repo = self.repo.clone();
        let sender = self.sender.clone();

        join_set.spawn(async move {
            let mut last_processed_event_id = ChangeEventId(0);
            let mut backoff = std::time::Duration::from_millis(100);
            let max_backoff = std::time::Duration::from_secs(30);

            loop {
                match sqlx::postgres::PgListener::connect(&connect_url).await {
                    Ok(mut listener) => {
                        if let Err(e) = listener.listen("registry_change").await {
                            tracing::warn!("Failed to LISTEN on registry_change: {e}");
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(max_backoff);
                            continue;
                        }

                        tracing::info!(
                            "PgListener connected, listening on registry_change channel"
                        );
                        backoff = std::time::Duration::from_millis(100);

                        // Catch up on any events that were written while we were disconnected
                        // or before this listener was started.
                        match repo.get_events_since(last_processed_event_id).await {
                            Ok(events) => {
                                for event in events {
                                    last_processed_event_id = event.event_id();
                                    let _ = sender.send(event);
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to catch up on events after connect: {e}, reconnecting"
                                );
                                tokio::time::sleep(backoff).await;
                                backoff = (backoff * 2).min(max_backoff);
                                continue;
                            }
                        }

                        loop {
                            match listener.recv().await {
                                Ok(_notification) => {
                                    match repo.get_events_since(last_processed_event_id).await {
                                        Ok(events) => {
                                            for event in events {
                                                last_processed_event_id = event.event_id();
                                                let _ = sender.send(event);
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "Failed to fetch events after NOTIFY: {e}"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "PgListener error on registry_change: {e}, reconnecting"
                                    );
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to connect PgListener: {e}");
                    }
                }

                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        });
    }
}

/// Replays events from the outbox since `cursor`. If the outbox returns no events
/// but newer events exist (i.e., older events were purged by retention cleanup),
/// sends a `cursor_expired` signal so the subscriber can flush its cache.
///
/// Returns:
/// - `Ok(new_cursor)` on success
/// - `Err(ReplayError::Disconnected)` if the receiver disconnected
/// - `Err(ReplayError::RepoError(e))` if the repo query failed
enum ReplayError {
    Disconnected,
    RepoError(String),
}

fn to_registry_invalidation_event(event: &RegistryChangeEvent) -> RegistryInvalidationEvent {
    let (event_id, payload) = match event {
        RegistryChangeEvent::DeploymentChanged {
            event_id,
            environment_id,
            deployment_revision_id,
        } => (
            *event_id,
            Payload::DeploymentChanged(DeploymentChangedEvent {
                environment_id: Some(EnvironmentId(*environment_id).into()),
                deployment_revision: *deployment_revision_id as u64,
            }),
        ),
        RegistryChangeEvent::DomainRegistrationChanged {
            event_id,
            environment_id,
            domains,
        } => (
            *event_id,
            Payload::DomainRegistrationChanged(DomainRegistrationChangedEvent {
                environment_id: Some(EnvironmentId(*environment_id).into()),
                domains: domains.clone(),
            }),
        ),
        RegistryChangeEvent::AccountTokensInvalidated {
            event_id,
            account_id,
        } => (
            *event_id,
            Payload::AccountTokensInvalidated(AccountTokensInvalidatedEvent {
                account_id: Some(AccountId(*account_id).into()),
            }),
        ),
        RegistryChangeEvent::EnvironmentPermissionsChanged {
            event_id,
            environment_id,
            grantee_account_id,
        } => (
            *event_id,
            Payload::PermissionsChanged(EnvironmentPermissionsChangedEvent {
                environment_id: Some(EnvironmentId(*environment_id).into()),
                grantee_account_id: Some(AccountId(*grantee_account_id).into()),
            }),
        ),
        RegistryChangeEvent::SecuritySchemeChanged {
            event_id,
            environment_id,
        } => (
            *event_id,
            Payload::SecuritySchemeChanged(SecuritySchemeChangedEvent {
                environment_id: Some(EnvironmentId(*environment_id).into()),
            }),
        ),
    };
    RegistryInvalidationEvent {
        event_id: event_id.0 as u64,
        payload: Some(payload),
    }
}

async fn replay_from_cursor(
    tx: &tokio::sync::mpsc::Sender<Result<RegistryInvalidationEvent, tonic::Status>>,
    repo: &dyn RegistryChangeRepo,
    cursor: ChangeEventId,
) -> Result<ChangeEventId, ReplayError> {
    match repo.get_events_since(cursor).await {
        Ok(events) if events.is_empty() => {
            if let Ok(Some(latest_id)) = repo.get_latest_event_id().await
                && latest_id.0 > cursor.0 + 1
            {
                if tx
                    .send(Ok(RegistryInvalidationEvent {
                        event_id: latest_id.0 as u64,
                        payload: Some(Payload::CursorExpired(CursorExpiredEvent {})),
                    }))
                    .await
                    .is_err()
                {
                    return Err(ReplayError::Disconnected);
                }
                return Ok(latest_id);
            }
            Ok(cursor)
        }
        Ok(events) if !events.is_empty() => {
            let mut cursor = cursor;
            // Gap detection: if first event is not cursor+1, cursor has expired
            if events[0].event_id().0 > cursor.0 + 1
                && tx
                    .send(Ok(RegistryInvalidationEvent {
                        event_id: events[0].event_id().0 as u64,
                        payload: Some(Payload::CursorExpired(CursorExpiredEvent {})),
                    }))
                    .await
                    .is_err()
            {
                return Err(ReplayError::Disconnected);
            }
            for event in &events {
                cursor = event.event_id();
                if tx
                    .send(Ok(to_registry_invalidation_event(event)))
                    .await
                    .is_err()
                {
                    return Err(ReplayError::Disconnected);
                }
            }
            Ok(cursor)
        }
        Ok(_) => Ok(cursor),
        Err(e) => Err(ReplayError::RepoError(e.to_string())),
    }
}

/// Produces a stream of registry invalidation events by replaying missed events from the
/// repository and then forwarding live events from the broadcast channel.
/// Includes all event types (deployment, token, permission changes).
pub fn subscribe_registry_invalidations(
    repo: Arc<dyn RegistryChangeRepo>,
    notifier: &dyn RegistryChangeNotifier,
    last_seen_event_id: Option<u64>,
) -> tokio_stream::wrappers::ReceiverStream<Result<RegistryInvalidationEvent, tonic::Status>> {
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let mut broadcast_rx = notifier.subscribe();

    tokio::spawn(async move {
        let mut cursor = ChangeEventId(last_seen_event_id.map(|id| id as i64).unwrap_or(0));

        if last_seen_event_id.is_some() {
            match replay_from_cursor(&tx, repo.as_ref(), cursor).await {
                Ok(new_cursor) => cursor = new_cursor,
                Err(ReplayError::Disconnected) => return,
                Err(ReplayError::RepoError(e)) => {
                    let _ = tx
                        .send(Err(tonic::Status::internal(format!(
                            "Failed to replay events: {e}"
                        ))))
                        .await;
                    return;
                }
            }
        }

        loop {
            match broadcast_rx.recv().await {
                Ok(event) => {
                    if event.event_id() > cursor {
                        cursor = event.event_id();
                        if tx
                            .send(Ok(to_registry_invalidation_event(&event)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    match replay_from_cursor(&tx, repo.as_ref(), cursor).await {
                        Ok(new_cursor) => cursor = new_cursor,
                        Err(ReplayError::Disconnected) => return,
                        Err(ReplayError::RepoError(_)) => break,
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    tokio_stream::wrappers::ReceiverStream::new(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;
    use uuid::Uuid;

    fn make_deployment_change_event(id: i64) -> RegistryChangeEvent {
        RegistryChangeEvent::DeploymentChanged {
            event_id: ChangeEventId(id),
            environment_id: Uuid::new_v4(),
            deployment_revision_id: id * 10,
        }
    }

    #[test]
    async fn test_broadcast_single_subscriber() {
        let notifier = LocalRegistryChangeNotifier::new(16);
        let mut rx = notifier.subscribe();

        let env_id = Uuid::new_v4();
        notifier.notify(RegistryChangeEvent::DeploymentChanged {
            event_id: ChangeEventId(1),
            environment_id: env_id,
            deployment_revision_id: 42,
        });

        let event = rx.recv().await.unwrap();
        assert_eq!(event.event_id(), ChangeEventId(1));
        assert!(matches!(
            event,
            RegistryChangeEvent::DeploymentChanged {
                environment_id,
                deployment_revision_id: 42,
                ..
            } if environment_id == env_id
        ));
    }

    #[test]
    async fn test_broadcast_multiple_subscribers() {
        let notifier = LocalRegistryChangeNotifier::new(16);
        let mut rx1 = notifier.subscribe();
        let mut rx2 = notifier.subscribe();

        let env_id = Uuid::new_v4();
        notifier.notify(RegistryChangeEvent::DeploymentChanged {
            event_id: ChangeEventId(5),
            environment_id: env_id,
            deployment_revision_id: 99,
        });

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();

        assert_eq!(e1.event_id(), ChangeEventId(5));
        assert_eq!(e2.event_id(), ChangeEventId(5));
        assert!(matches!(
            e1,
            RegistryChangeEvent::DeploymentChanged {
                environment_id,
                deployment_revision_id: 99,
                ..
            } if environment_id == env_id
        ));
        assert!(matches!(
            e2,
            RegistryChangeEvent::DeploymentChanged {
                environment_id,
                deployment_revision_id: 99,
                ..
            } if environment_id == env_id
        ));
    }

    #[test]
    fn test_notify_no_receivers() {
        let notifier = LocalRegistryChangeNotifier::new(16);
        // Should not panic even with no subscribers
        notifier.notify(RegistryChangeEvent::DeploymentChanged {
            event_id: ChangeEventId(10),
            environment_id: Uuid::new_v4(),
            deployment_revision_id: 7,
        });
    }

    #[test]
    fn test_subscribe_after_notify() {
        let notifier = LocalRegistryChangeNotifier::new(16);

        notifier.notify(RegistryChangeEvent::DeploymentChanged {
            event_id: ChangeEventId(20),
            environment_id: Uuid::new_v4(),
            deployment_revision_id: 100,
        });

        // Subscribe after the event was sent
        let mut rx = notifier.subscribe();

        // The new subscriber should not see the old event
        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "expected empty channel, but got a message"
        );
    }

    // ── Mock repo for subscribe_registry_invalidations tests ──

    struct MockRegistryChangeRepo {
        events: std::sync::Mutex<Vec<RegistryChangeEvent>>,
        latest_event_id_override: std::sync::Mutex<Option<ChangeEventId>>,
    }

    impl MockRegistryChangeRepo {
        fn new() -> Self {
            Self {
                events: std::sync::Mutex::new(Vec::new()),
                latest_event_id_override: std::sync::Mutex::new(None),
            }
        }

        fn push(&self, event: RegistryChangeEvent) {
            self.events.lock().unwrap().push(event);
        }

        fn set_latest_event_id_override(&self, id: ChangeEventId) {
            *self.latest_event_id_override.lock().unwrap() = Some(id);
        }
    }

    #[async_trait::async_trait]
    impl RegistryChangeRepo for MockRegistryChangeRepo {
        async fn record_change_event(
            &self,
            event: &crate::repo::registry_change::NewRegistryChangeEvent,
        ) -> golem_service_base::repo::RepoResult<ChangeEventId> {
            use crate::repo::registry_change::RegistryEventType;
            let mut events = self.events.lock().unwrap();
            let id = ChangeEventId(events.len() as i64 + 1);
            let change_event = match event.event_type {
                RegistryEventType::DeploymentChanged => RegistryChangeEvent::DeploymentChanged {
                    event_id: id,
                    environment_id: event.environment_id.unwrap_or_default(),
                    deployment_revision_id: event.deployment_revision_id.unwrap_or_default(),
                },
                RegistryEventType::AccountTokensInvalidated => {
                    RegistryChangeEvent::AccountTokensInvalidated {
                        event_id: id,
                        account_id: event.account_id.unwrap_or_default(),
                    }
                }
                RegistryEventType::EnvironmentPermissionsChanged => {
                    RegistryChangeEvent::EnvironmentPermissionsChanged {
                        event_id: id,
                        environment_id: event.environment_id.unwrap_or_default(),
                        grantee_account_id: event.grantee_account_id.unwrap_or_default(),
                    }
                }
                RegistryEventType::DomainRegistrationChanged => {
                    RegistryChangeEvent::DomainRegistrationChanged {
                        event_id: id,
                        environment_id: event.environment_id.unwrap_or_default(),
                        domains: event.domains.clone(),
                    }
                }
            };
            events.push(change_event);
            Ok(id)
        }

        async fn get_events_since(
            &self,
            last_seen_event_id: ChangeEventId,
        ) -> golem_service_base::repo::RepoResult<Vec<RegistryChangeEvent>> {
            let events = self.events.lock().unwrap();
            Ok(events
                .iter()
                .filter(|e| e.event_id() > last_seen_event_id)
                .cloned()
                .collect())
        }

        async fn get_latest_event_id(
            &self,
        ) -> golem_service_base::repo::RepoResult<Option<ChangeEventId>> {
            if let Some(id) = *self.latest_event_id_override.lock().unwrap() {
                return Ok(Some(id));
            }
            let events = self.events.lock().unwrap();
            Ok(events.last().map(|e| e.event_id()))
        }

        async fn cleanup_old_events(
            &self,
            _retention_seconds: i64,
        ) -> golem_service_base::repo::RepoResult<u64> {
            Ok(0)
        }
    }

    #[test]
    async fn test_subscribe_replays_missed_events() {
        use tokio_stream::StreamExt;

        let repo = Arc::new(MockRegistryChangeRepo::new());
        repo.push(make_deployment_change_event(1));
        repo.push(make_deployment_change_event(2));
        repo.push(make_deployment_change_event(3));

        let notifier = LocalRegistryChangeNotifier::new(16);
        let mut stream = subscribe_registry_invalidations(repo, &notifier, Some(1));

        let e1 = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("error");
        assert_eq!(e1.event_id, 2);

        let e2 = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("error");
        assert_eq!(e2.event_id, 3);
    }

    #[test]
    async fn test_subscribe_forwards_live_events() {
        use tokio_stream::StreamExt;

        let repo = Arc::new(MockRegistryChangeRepo::new());
        let notifier = LocalRegistryChangeNotifier::new(16);
        let mut stream = subscribe_registry_invalidations(repo, &notifier, None);

        // Give the spawned task time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let ev = make_deployment_change_event(10);
        notifier.notify(ev);

        let received = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("error");
        assert_eq!(received.event_id, 10);
    }

    #[test]
    async fn test_subscribe_deduplicates_events() {
        use tokio_stream::StreamExt;

        let repo = Arc::new(MockRegistryChangeRepo::new());
        let ev1 = make_deployment_change_event(1);
        let ev2 = make_deployment_change_event(2);
        repo.push(ev1.clone());
        repo.push(ev2.clone());

        let notifier = LocalRegistryChangeNotifier::new(16);
        // Subscribe with cursor 0, so replay returns events 1 and 2
        let mut stream = subscribe_registry_invalidations(repo, &notifier, Some(0));

        // Also push the same events through the notifier (simulating overlap)
        notifier.notify(ev1);
        notifier.notify(ev2);
        // Push a new event to confirm the stream advances
        let ev3 = make_deployment_change_event(3);
        notifier.notify(ev3);

        let mut received_ids = Vec::new();
        for _ in 0..3 {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .expect("timeout")
                .expect("stream ended")
                .expect("error");
            received_ids.push(ev.event_id);
        }

        // Should see exactly [1, 2, 3] with no duplicates
        assert_eq!(received_ids, vec![1, 2, 3]);
    }

    #[test]
    async fn test_subscribe_handles_lag_recovery() {
        use tokio_stream::StreamExt;

        let repo = Arc::new(MockRegistryChangeRepo::new());
        // Pre-populate events 1..=5 so replay can find them
        for i in 1..=5 {
            repo.push(make_deployment_change_event(i));
        }

        // Capacity 2 — will lag when we burst
        let notifier = LocalRegistryChangeNotifier::new(2);
        let mut stream = subscribe_registry_invalidations(repo, &notifier, None);

        // Give the spawned task time to start listening
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Burst 5 events to overflow the capacity-2 broadcast
        for i in 1..=5 {
            notifier.notify(make_deployment_change_event(i));
        }

        // The stream should eventually deliver all 5 events (via lag recovery replay)
        let mut seen = std::collections::HashSet::new();
        for _ in 0..5 {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .expect("timeout")
                .expect("stream ended")
                .expect("error");
            seen.insert(ev.event_id);
        }

        for i in 1..=5u64 {
            assert!(seen.contains(&i), "missing event_id {i}");
        }
    }

    #[test]
    async fn test_subscribe_cursor_expired() {
        use tokio_stream::StreamExt;

        let repo = Arc::new(MockRegistryChangeRepo::new());
        // No events in the repo (simulating retention cleanup wiped them),
        // but latest_event_id is 100 (events existed and were cleaned up).
        repo.set_latest_event_id_override(ChangeEventId(100));

        let notifier = LocalRegistryChangeNotifier::new(16);
        // Subscribe with cursor 5 — events 6..100 were purged
        let mut stream = subscribe_registry_invalidations(repo, &notifier, Some(5));

        let ev = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("error");
        assert!(
            matches!(ev.payload, Some(Payload::CursorExpired(_))),
            "expected CursorExpired payload, got {:?}",
            ev.payload
        );
        assert_eq!(ev.event_id, 100);
    }

    #[test]
    async fn test_lagged_receiver() {
        // Create notifier with capacity of 2
        let notifier = LocalRegistryChangeNotifier::new(2);
        let mut rx = notifier.subscribe();

        // Send 3 events to overflow the capacity-2 buffer
        for i in 1..=3 {
            notifier.notify(RegistryChangeEvent::DeploymentChanged {
                event_id: ChangeEventId(i),
                environment_id: Uuid::new_v4(),
                deployment_revision_id: i * 10,
            });
        }

        // First recv should report Lagged
        let result = rx.recv().await;
        assert!(
            matches!(result, Err(broadcast::error::RecvError::Lagged(_))),
            "expected Lagged error, got {result:?}"
        );

        // After lagged, we can still receive the remaining buffered events
        let event = rx.recv().await.unwrap();
        // With capacity=2, after lag recovery we get the oldest surviving event
        assert!(
            event.event_id() >= ChangeEventId(2),
            "expected surviving event, got {}",
            event.event_id().0
        );
    }
}
