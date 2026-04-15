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

use crate::repo::registry_change::{
    ChangeEventId, RegistryChangeEvent, RegistryChangeRepo, RequiresNotificationSignal,
};
use golem_api_grpc::proto::golem::registry::v1::AgentSecretChangedEvent;
use golem_api_grpc::proto::golem::registry::v1::{
    AccountTokensInvalidatedEvent, CursorExpiredEvent, DeploymentChangedEvent,
    DomainRegistrationChangedEvent, EnvironmentPermissionsChangedEvent, RegistryInvalidationEvent,
    ResourceDefinitionChangedEvent, RetryPolicyChangedEvent, SecuritySchemeChangedEvent,
    registry_invalidation_event::Payload,
};
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::ResourceDefinitionId;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc};

/// Manages registry change event broadcasting. Delivery is at-least-once;
/// subscribers must deduplicate by event_id.
pub trait RegistryChangeNotifier: Send + Sync {
    /// Signal that new events may be available.
    fn signal_new_events_available(&self);

    /// Get a new broadcast receiver for subscribing to live events.
    fn subscribe(&self) -> broadcast::Receiver<RegistryChangeEvent>;

    /// Start any background tasks needed by this implementation (e.g., PgListener
    /// or local signal processing worker).
    fn start_background_tasks(
        &self,
        _join_set: &mut tokio::task::JoinSet<Result<(), anyhow::Error>>,
    ) {
        // Default no-op
    }
}

impl<T> RegistryChangeNotifier for Arc<T>
where
    T: RegistryChangeNotifier + ?Sized,
{
    fn signal_new_events_available(&self) {
        (**self).signal_new_events_available();
    }

    fn subscribe(&self) -> broadcast::Receiver<RegistryChangeEvent> {
        (**self).subscribe()
    }

    fn start_background_tasks(
        &self,
        join_set: &mut tokio::task::JoinSet<Result<(), anyhow::Error>>,
    ) {
        (**self).start_background_tasks(join_set);
    }
}

pub trait RequiresNotificationSignalExt<T> {
    fn signal_new_events_available(self, notifier: &dyn RegistryChangeNotifier) -> T;
}

impl<T> RequiresNotificationSignalExt<T> for RequiresNotificationSignal<T> {
    fn signal_new_events_available(self, notifier: &dyn RegistryChangeNotifier) -> T {
        notifier.signal_new_events_available();
        self.into_inner_after_signal()
    }
}

/// Local in-process notifier for single-node SQLite deployments (e.g.: golem server run)
/// Signal calls enqueue local processing and a background worker fetches and broadcasts
/// concrete registry change events.
pub struct SqliteRegistryChangeNotifier {
    sender: broadcast::Sender<RegistryChangeEvent>,
    repo: Arc<dyn RegistryChangeRepo>,
    signal_tx: mpsc::Sender<()>,
    signal_rx: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
}

impl SqliteRegistryChangeNotifier {
    pub fn new(capacity: usize, repo: Arc<dyn RegistryChangeRepo>) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        let (signal_tx, signal_rx) = mpsc::channel(1);
        Self {
            sender,
            repo,
            signal_tx,
            signal_rx: Arc::new(Mutex::new(Some(signal_rx))),
        }
    }
}

impl RegistryChangeNotifier for SqliteRegistryChangeNotifier {
    fn signal_new_events_available(&self) {
        if let Err(err) = self.signal_tx.try_send(()) {
            match err {
                mpsc::error::TrySendError::Full(_) => {
                    // A signal is already queued and the worker drains/replays from the outbox,
                    // so dropping this duplicate signal is safe.
                }
                mpsc::error::TrySendError::Closed(_) => {
                    tracing::warn!("Local notifier worker is not running");
                }
            }
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<RegistryChangeEvent> {
        self.sender.subscribe()
    }

    fn start_background_tasks(
        &self,
        join_set: &mut tokio::task::JoinSet<Result<(), anyhow::Error>>,
    ) {
        let repo = self.repo.clone();
        let sender = self.sender.clone();
        let signal_rx = self.signal_rx.clone();

        join_set.spawn(async move {
            let Some(mut signal_rx) = signal_rx.lock().await.take() else {
                return Ok(());
            };

            let mut last_processed_event_id = ChangeEventId(0);

            while signal_rx.recv().await.is_some() {
                while signal_rx.try_recv().is_ok() {}

                match repo.get_events_since(last_processed_event_id).await {
                    Ok(events) => {
                        for event in events {
                            last_processed_event_id = event.event_id();
                            let _ = sender.send(event);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to fetch events after signal: {e}");
                    }
                }
            }

            Ok(())
        });
    }
}

/// Postgres-backed notifier that uses LISTEN/NOTIFY for cross-node propagation.
/// Background listener tasks fetch outbox events and broadcast them internally.
/// External signal calls are ignored for Postgres.
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
    fn signal_new_events_available(&self) {
        // Postgres path relies on LISTEN/NOTIFY and background replay.
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
            current_deployment_revision_id,
        } => (
            *event_id,
            Payload::DeploymentChanged(DeploymentChangedEvent {
                environment_id: Some(EnvironmentId(*environment_id).into()),
                deployment_revision: *deployment_revision_id as u64,
                current_deployment_revision: *current_deployment_revision_id as u64,
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
        RegistryChangeEvent::RetryPolicyChanged {
            event_id,
            environment_id,
        } => (
            *event_id,
            Payload::RetryPolicyChanged(RetryPolicyChangedEvent {
                environment_id: Some(EnvironmentId(*environment_id).into()),
            }),
        ),
        RegistryChangeEvent::ResourceDefinitionChanged {
            event_id,
            environment_id,
            resource_definition_id,
            resource_name,
        } => (
            *event_id,
            Payload::ResourceDefinitionChanged(ResourceDefinitionChangedEvent {
                environment_id: Some(EnvironmentId(*environment_id).into()),
                resource_definition_id: Some(ResourceDefinitionId(*resource_definition_id).into()),
                resource_name: resource_name.clone(),
            }),
        ),
        RegistryChangeEvent::AgentSecretChanged {
            event_id,
            environment_id,
        } => (
            *event_id,
            Payload::AgentSecretChanged(AgentSecretChangedEvent {
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
/// If `last_seen_event_id` is omitted, the stream starts from the latest known event id
/// (live-only mode).
/// Includes all event types (deployment, token, permission changes).
pub fn subscribe_registry_invalidations(
    repo: Arc<dyn RegistryChangeRepo>,
    notifier: &dyn RegistryChangeNotifier,
    last_seen_event_id: Option<u64>,
) -> tokio_stream::wrappers::ReceiverStream<Result<RegistryInvalidationEvent, tonic::Status>> {
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let mut broadcast_rx = notifier.subscribe();

    tokio::spawn(async move {
        let mut cursor = if let Some(last_seen_event_id) = last_seen_event_id {
            ChangeEventId(last_seen_event_id as i64)
        } else {
            match repo.get_latest_event_id().await {
                Ok(Some(latest_id)) => latest_id,
                Ok(None) => ChangeEventId(0),
                Err(e) => {
                    let _ = tx
                        .send(Err(tonic::Status::internal(format!(
                            "Failed to initialize live cursor: {e}"
                        ))))
                        .await;
                    return;
                }
            }
        };

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
            current_deployment_revision_id: id,
        }
    }

    #[test]
    async fn test_broadcast_single_subscriber() {
        let repo = Arc::new(MockRegistryChangeRepo::new());
        let notifier = SqliteRegistryChangeNotifier::new(16, repo.clone());
        let mut join_set = tokio::task::JoinSet::new();
        notifier.start_background_tasks(&mut join_set);
        let mut rx = notifier.subscribe();

        let env_id = Uuid::new_v4();
        repo.push(RegistryChangeEvent::DeploymentChanged {
            event_id: ChangeEventId(1),
            environment_id: env_id,
            deployment_revision_id: 42,
            current_deployment_revision_id: 1,
        });
        notifier.signal_new_events_available();

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
        join_set.abort_all();
    }

    #[test]
    async fn test_broadcast_multiple_subscribers() {
        let repo = Arc::new(MockRegistryChangeRepo::new());
        let notifier = SqliteRegistryChangeNotifier::new(16, repo.clone());
        let mut join_set = tokio::task::JoinSet::new();
        notifier.start_background_tasks(&mut join_set);
        let mut rx1 = notifier.subscribe();
        let mut rx2 = notifier.subscribe();

        let env_id = Uuid::new_v4();
        repo.push(RegistryChangeEvent::DeploymentChanged {
            event_id: ChangeEventId(5),
            environment_id: env_id,
            deployment_revision_id: 99,
            current_deployment_revision_id: 5,
        });
        notifier.signal_new_events_available();

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
        join_set.abort_all();
    }

    #[test]
    fn test_signal_no_receivers() {
        let repo = Arc::new(MockRegistryChangeRepo::new());
        let notifier = SqliteRegistryChangeNotifier::new(16, repo);
        // Should not panic even with no subscribers
        notifier.signal_new_events_available();
    }

    #[test]
    fn test_subscribe_after_signal() {
        let repo = Arc::new(MockRegistryChangeRepo::new());
        let notifier = SqliteRegistryChangeNotifier::new(16, repo);

        notifier.signal_new_events_available();

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
        block_next_get_events_since: std::sync::Mutex<Option<Arc<BlockingGetEventsSince>>>,
        get_events_since_call_count: std::sync::atomic::AtomicUsize,
        get_events_since_called: tokio::sync::Notify,
    }

    struct BlockingGetEventsSince {
        entered: tokio::sync::Semaphore,
        release: tokio::sync::Semaphore,
    }

    impl BlockingGetEventsSince {
        fn new() -> Self {
            Self {
                entered: tokio::sync::Semaphore::new(0),
                release: tokio::sync::Semaphore::new(0),
            }
        }

        async fn wait_until_entered(&self) {
            self.entered
                .acquire()
                .await
                .expect("blocking semaphore closed")
                .forget();
        }

        fn unblock(&self) {
            self.release.add_permits(1);
        }
    }

    impl MockRegistryChangeRepo {
        fn new() -> Self {
            Self {
                events: std::sync::Mutex::new(Vec::new()),
                latest_event_id_override: std::sync::Mutex::new(None),
                block_next_get_events_since: std::sync::Mutex::new(None),
                get_events_since_call_count: std::sync::atomic::AtomicUsize::new(0),
                get_events_since_called: tokio::sync::Notify::new(),
            }
        }

        fn push(&self, event: RegistryChangeEvent) {
            self.events.lock().unwrap().push(event);
        }

        fn set_latest_event_id_override(&self, id: ChangeEventId) {
            *self.latest_event_id_override.lock().unwrap() = Some(id);
        }

        fn block_next_get_events_since(&self) -> Arc<BlockingGetEventsSince> {
            let blocker = Arc::new(BlockingGetEventsSince::new());
            *self.block_next_get_events_since.lock().unwrap() = Some(blocker.clone());
            blocker
        }

        async fn wait_for_get_events_since_calls(&self, target: usize) {
            loop {
                let notified = self.get_events_since_called.notified();
                if self
                    .get_events_since_call_count
                    .load(std::sync::atomic::Ordering::SeqCst)
                    >= target
                {
                    return;
                }
                notified.await;
            }
        }
    }

    #[async_trait::async_trait]
    impl RegistryChangeRepo for MockRegistryChangeRepo {
        async fn get_events_since(
            &self,
            last_seen_event_id: ChangeEventId,
        ) -> golem_service_base::repo::RepoResult<Vec<RegistryChangeEvent>> {
            self.get_events_since_call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.get_events_since_called.notify_waiters();

            let blocker = self.block_next_get_events_since.lock().unwrap().take();
            if let Some(blocker) = blocker {
                blocker.entered.add_permits(1);
                blocker
                    .release
                    .acquire()
                    .await
                    .expect("blocking semaphore closed")
                    .forget();
            }

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

        let notifier = SqliteRegistryChangeNotifier::new(16, repo.clone());
        let mut join_set = tokio::task::JoinSet::new();
        notifier.start_background_tasks(&mut join_set);
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
        join_set.abort_all();
    }

    #[test]
    async fn test_subscribe_forwards_live_events() {
        use tokio_stream::StreamExt;

        let repo = Arc::new(MockRegistryChangeRepo::new());
        let notifier = SqliteRegistryChangeNotifier::new(16, repo.clone());
        let mut join_set = tokio::task::JoinSet::new();
        notifier.start_background_tasks(&mut join_set);
        let mut stream = subscribe_registry_invalidations(repo.clone(), &notifier, None);

        // Give the spawned task time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        repo.push(make_deployment_change_event(10));
        notifier.signal_new_events_available();

        let received = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("error");
        assert_eq!(received.event_id, 10);
        join_set.abort_all();
    }

    #[test]
    async fn test_subscribe_none_is_live_only() {
        use tokio_stream::StreamExt;

        let repo = Arc::new(MockRegistryChangeRepo::new());
        repo.push(make_deployment_change_event(1));
        repo.push(make_deployment_change_event(2));

        let notifier = SqliteRegistryChangeNotifier::new(16, repo.clone());
        let mut join_set = tokio::task::JoinSet::new();
        notifier.start_background_tasks(&mut join_set);
        let mut stream = subscribe_registry_invalidations(repo.clone(), &notifier, None);

        // Historical events exist but should not be replayed in live-only mode.
        let no_event =
            tokio::time::timeout(std::time::Duration::from_millis(150), stream.next()).await;
        assert!(
            no_event.is_err(),
            "expected no replayed event in live-only mode"
        );

        repo.push(make_deployment_change_event(3));
        notifier.signal_new_events_available();

        let received = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("timeout")
            .expect("stream ended")
            .expect("error");
        assert_eq!(received.event_id, 3);
        join_set.abort_all();
    }

    #[test]
    async fn test_subscribe_deduplicates_events() {
        use tokio_stream::StreamExt;

        let repo = Arc::new(MockRegistryChangeRepo::new());
        repo.push(make_deployment_change_event(1));
        repo.push(make_deployment_change_event(2));

        let notifier = SqliteRegistryChangeNotifier::new(16, repo.clone());
        let mut join_set = tokio::task::JoinSet::new();
        notifier.start_background_tasks(&mut join_set);
        // Subscribe with cursor 0, so replay returns events 1 and 2
        let mut stream = subscribe_registry_invalidations(repo.clone(), &notifier, Some(0));

        // Also signal replay through the notifier (simulating overlap)
        notifier.signal_new_events_available();
        notifier.signal_new_events_available();
        // Push a new event to confirm the stream advances
        repo.push(make_deployment_change_event(3));
        notifier.signal_new_events_available();

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
        join_set.abort_all();
    }

    #[test]
    async fn test_subscribe_handles_lag_recovery() {
        use tokio_stream::StreamExt;

        let repo = Arc::new(MockRegistryChangeRepo::new());
        // Pre-populate historical events; live-only subscriptions should not replay these.
        for i in 1..=5 {
            repo.push(make_deployment_change_event(i));
        }

        // Capacity 2 — will lag when we burst
        let notifier = SqliteRegistryChangeNotifier::new(2, repo.clone());
        let mut join_set = tokio::task::JoinSet::new();
        notifier.start_background_tasks(&mut join_set);
        let mut stream = subscribe_registry_invalidations(repo.clone(), &notifier, None);

        // Give the spawned task time to start listening
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Add live events after subscription and burst signals to overflow capacity.
        for i in 6..=10 {
            repo.push(make_deployment_change_event(i));
            notifier.signal_new_events_available();
        }

        // The stream should eventually deliver all live events via lag-recovery replay.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..5 {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                .await
                .expect("timeout")
                .expect("stream ended")
                .expect("error");
            seen.insert(ev.event_id);
        }

        for i in 6..=10u64 {
            assert!(seen.contains(&i), "missing event_id {i}");
        }
        join_set.abort_all();
    }

    #[test]
    async fn test_subscribe_cursor_expired() {
        use tokio_stream::StreamExt;

        let repo = Arc::new(MockRegistryChangeRepo::new());
        // No events in the repo (simulating retention cleanup wiped them),
        // but latest_event_id is 100 (events existed and were cleaned up).
        repo.set_latest_event_id_override(ChangeEventId(100));

        let notifier = SqliteRegistryChangeNotifier::new(16, repo.clone());
        let mut join_set = tokio::task::JoinSet::new();
        notifier.start_background_tasks(&mut join_set);
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
        join_set.abort_all();
    }

    #[test]
    async fn test_lagged_receiver() {
        // Block the first repo fetch so the worker is definitely waiting in the
        // notifier path before we enqueue the burst of events.
        let repo = Arc::new(MockRegistryChangeRepo::new());
        let first_fetch_blocker = repo.block_next_get_events_since();
        let notifier = SqliteRegistryChangeNotifier::new(2, repo.clone());
        let mut join_set = tokio::task::JoinSet::new();
        notifier.start_background_tasks(&mut join_set);
        let mut rx = notifier.subscribe();

        notifier.signal_new_events_available();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            first_fetch_blocker.wait_until_entered(),
        )
        .await
        .expect("worker never entered get_events_since");

        // Queue 3 live events while the worker is blocked. Once released, the worker
        // fetches and broadcasts the whole burst, overflowing the capacity-2 channel.
        for i in 1..=3 {
            repo.push(make_deployment_change_event(i));
            notifier.signal_new_events_available();
        }

        first_fetch_blocker.unblock();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            repo.wait_for_get_events_since_calls(2),
        )
        .await
        .expect("worker did not finish the burst replay");

        assert!(matches!(
            rx.recv().await,
            Err(broadcast::error::RecvError::Lagged(1))
        ));

        let first_surviving_event = rx.recv().await.unwrap();
        let second_surviving_event = rx.recv().await.unwrap();

        assert_eq!(first_surviving_event.event_id(), ChangeEventId(2));
        assert_eq!(second_surviving_event.event_id(), ChangeEventId(3));
        join_set.abort_all();
    }
}
