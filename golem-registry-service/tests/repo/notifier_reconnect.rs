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

//! The PostgreSQL notifier and the account tree cache across a lost listener connection.
//!
//! The listener reaches PostgreSQL through a local proxy that the test can cut. Cutting closes
//! the proxied connections cleanly, so the listener sees end of stream and reconnects on its
//! own. The proxy holds the reconnect until the test has committed a change, so the change's
//! notification is lost and no later write notifies again.
//!
//! The journal expiry test empties the journal, so it uses its own schema: other tests share
//! the database but not its tables. Notifications are database-wide, so the listener still
//! receives theirs.

use crate::repo::postgres::{PostgresDb, make_pool};
use futures::FutureExt;
use golem_common::config::DbPostgresConfig;
use golem_common::model::account_path::{AccountPath, AccountSegment};
use golem_registry_service::repo::account_tree::{AccountTreeRepo, DbAccountTreeRepo};
use golem_registry_service::repo::model::account_tree::{
    AccountLifecycleState, AccountNodeChange, AccountNodeRecord, AccountTreeRepoError,
    effective_state,
};
use golem_registry_service::repo::model::new_repo_uuid;
use golem_registry_service::repo::registry_change::{
    ChangeEventId, DbRegistryChangeRepo, NewRegistryChangeEvent, RegistryChangeEvent,
    RegistryChangeRepo,
};
use golem_registry_service::services::account_tree_cache::AccountTreeCache;
use golem_registry_service::services::registry_change_notifier::{
    PostgresRegistryChangeNotifier, RegistryChangeNotifier,
};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::{Pool, PoolApi};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

struct CuttableProxy {
    port: u16,
    hold: watch::Sender<bool>,
    cut: watch::Sender<u64>,
    open: Arc<AtomicUsize>,
    _accept: tokio::task::JoinHandle<()>,
}

impl CuttableProxy {
    async fn start(upstream_port: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (hold, hold_rx) = watch::channel(false);
        let (cut, cut_rx) = watch::channel(0u64);
        let open = Arc::new(AtomicUsize::new(0));
        let accept = tokio::spawn({
            let open = open.clone();
            async move {
                loop {
                    let Ok((mut client, _)) = listener.accept().await else {
                        return;
                    };
                    let mut hold_rx = hold_rx.clone();
                    let mut cut_rx = cut_rx.clone();
                    let open = open.clone();
                    tokio::spawn(async move {
                        // A held connection is accepted but not forwarded until release.
                        let _ = hold_rx.wait_for(|held| !*held).await;
                        let Ok(mut upstream) =
                            TcpStream::connect(("127.0.0.1", upstream_port)).await
                        else {
                            return;
                        };
                        cut_rx.borrow_and_update();
                        open.fetch_add(1, Ordering::SeqCst);
                        tokio::select! {
                            _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
                            _ = cut_rx.changed() => {}
                        }
                        open.fetch_sub(1, Ordering::SeqCst);
                        // A clean close: the client reads end of stream.
                        let _ = client.shutdown().await;
                    });
                }
            }
        });
        Self {
            port,
            hold,
            cut,
            open,
            _accept: accept,
        }
    }

    fn open_connections(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }

    /// Closes every forwarded connection and holds new ones until [`Self::release`].
    async fn cut_and_hold(&self) {
        self.hold.send_replace(true);
        self.cut.send_modify(|generation| *generation += 1);
        wait_until(Duration::from_secs(10), || self.open_connections() == 0).await;
    }

    fn release(&self) {
        self.hold.send_replace(false);
    }
}

async fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    condition()
}

struct Fixture {
    proxy: CuttableProxy,
    tree: Arc<dyn AccountTreeRepo>,
    journal: Arc<dyn RegistryChangeRepo>,
    cache: Arc<AccountTreeCache>,
    acme: AccountNodeRecord,
    leaf: AccountNodeRecord,
    events: broadcast::Receiver<RegistryChangeEvent>,
    _tasks: tokio::task::JoinSet<Result<(), anyhow::Error>>,
    _invalidation: tokio::task::JoinHandle<()>,
}

fn segment(value: &str) -> AccountSegment {
    AccountSegment::from_str(value).unwrap()
}

async fn root(tree: &Arc<dyn AccountTreeRepo>) -> AccountNodeRecord {
    if let Some(root) = tree.resolve_path(&AccountPath::root()).await.unwrap().pop() {
        return root;
    }
    match tree.create_root(Uuid::new_v4(), None, Uuid::new_v4()).await {
        Ok(root) => root,
        Err(AccountTreeRepoError::RootExists) => tree
            .resolve_path(&AccountPath::root())
            .await
            .unwrap()
            .pop()
            .unwrap(),
        Err(err) => panic!("failed to create root: {err}"),
    }
}

async fn suspend(tree: &Arc<dyn AccountTreeRepo>, node: &AccountNodeRecord) {
    // The returned signal is for the SQLite notifier; PostgreSQL notifies by itself.
    let _ = tree
        .update(
            node.account_id,
            node.current_revision as u64,
            AccountNodeChange {
                segment: None,
                display_name: node.display_name.clone(),
                own_state: AccountLifecycleState::Suspended,
            },
            Uuid::new_v4(),
        )
        .await
        .unwrap();
}

async fn leaf_state(cache: &AccountTreeCache, leaf: Uuid) -> AccountLifecycleState {
    effective_state(&cache.ancestry(leaf).await.unwrap().unwrap())
}

fn spawn_cache_invalidation(
    cache: Arc<AccountTreeCache>,
    notifier: &PostgresRegistryChangeNotifier,
) -> tokio::task::JoinHandle<()> {
    let events = notifier.subscribe();
    let resyncs = notifier.subscribe_resyncs();
    tokio::spawn(async move { cache.run_invalidation(events, resyncs).await })
}

/// A warm cache behind a notifier whose listener goes through the proxy, with the listener
/// shown to be live.
async fn fixture(config: &DbPostgresConfig, pool: &PostgresPool) -> Fixture {
    let proxy = CuttableProxy::start(config.port).await;
    let mut proxied = config.clone();
    proxied.port = proxy.port;

    let journal: Arc<dyn RegistryChangeRepo> = Arc::new(DbRegistryChangeRepo::new(pool.clone()));
    let notifier = PostgresRegistryChangeNotifier::new(4096, journal.clone(), &proxied);
    let mut tasks = tokio::task::JoinSet::new();
    notifier.start_background_tasks(&mut tasks);

    let tree: Arc<dyn AccountTreeRepo> = Arc::new(DbAccountTreeRepo::logged(pool.clone()));
    let cache = Arc::new(AccountTreeCache::new(
        tree.clone(),
        64,
        crate::repo::account_tree::LONG_MAX_AGE,
    ));
    let invalidation = spawn_cache_invalidation(cache.clone(), &notifier);
    let events = notifier.subscribe();

    let root = root(&tree).await;
    let top = segment(&format!("n{}", new_repo_uuid().simple()));
    let acme = tree
        .create_child(Uuid::new_v4(), root.account_id, top, None, Uuid::new_v4())
        .await
        .unwrap();
    let leaf = tree
        .create_child(
            Uuid::new_v4(),
            acme.account_id,
            segment("leaf"),
            None,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    assert!(wait_until(Duration::from_secs(10), || proxy.open_connections() >= 1).await);

    // A probe change on another node shows that notifications reach the cache.
    let probe = tree
        .create_child(
            Uuid::new_v4(),
            acme.account_id,
            segment("probe"),
            None,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    cache.ancestry(probe.account_id).await.unwrap().unwrap();
    suspend(&tree, &probe).await;
    let probe_seen = {
        let start = Instant::now();
        loop {
            if leaf_state(&cache, probe.account_id).await == AccountLifecycleState::Suspended {
                break true;
            }
            if start.elapsed() > Duration::from_secs(10) {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    assert!(
        probe_seen,
        "the live notification path did not reach the cache"
    );

    assert!(leaf_state(&cache, leaf.account_id).await == AccountLifecycleState::Active);
    Fixture {
        proxy,
        tree,
        journal,
        cache,
        acme,
        leaf,
        events,
        _tasks: tasks,
        _invalidation: invalidation,
    }
}

async fn becomes_suspended(cache: &AccountTreeCache, leaf: Uuid, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if leaf_state(cache, leaf).await == AccountLifecycleState::Suspended {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A change committed while the listener is disconnected reaches the warm cache after the
/// listener reconnects, without any later write.
pub async fn test_notifier_replays_after_listener_reconnect(db: &PostgresDb) {
    let mut fixture = fixture(&db.config, &db.pool).await;
    fixture.proxy.cut_and_hold().await;
    suspend(&fixture.tree, &fixture.acme).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    fixture.proxy.release();

    assert!(
        becomes_suspended(
            &fixture.cache,
            fixture.leaf.account_id,
            Duration::from_secs(10)
        )
        .await,
        "the cache kept the state from before the disconnect"
    );
    // Subscribers other than the cache, such as invalidation streams, receive the lost event.
    let acme = fixture.acme.account_id;
    let delivered = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match fixture.events.recv().await {
                Ok(RegistryChangeEvent::AccountChanged { account_id, .. })
                    if account_id == acme =>
                {
                    break;
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => panic!("notifier closed"),
            }
        }
    })
    .await;
    assert!(
        delivered.is_ok(),
        "the notifier did not broadcast the lost event"
    );
}

/// When the change committed during the disconnect is no longer in the journal but a later
/// event is, the notifier cannot replay it and the cache resets instead.
pub async fn test_notifier_resets_cache_after_journal_gap(db: &PostgresDb) {
    let fixture = fixture(&db.config, &db.pool).await;
    fixture.proxy.cut_and_hold().await;
    suspend(&fixture.tree, &fixture.acme).await;
    let lost = fixture
        .journal
        .get_latest_event_id()
        .await
        .unwrap()
        .unwrap();
    record_unrelated_event(db).await;
    db.pool
        .with_rw("notifier_reconnect_test", "purge_event")
        .execute(sqlx::query("DELETE FROM registry_change_events WHERE event_id = $1").bind(lost.0))
        .await
        .unwrap();
    assert!(
        fixture
            .journal
            .get_events_since(ChangeEventId(lost.0 - 1))
            .await
            .unwrap()
            .iter()
            .all(|event| event.event_id() != lost)
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    fixture.proxy.release();

    assert!(
        becomes_suspended(
            &fixture.cache,
            fixture.leaf.account_id,
            Duration::from_secs(10)
        )
        .await,
        "the cache kept the state from before the disconnect"
    );
}

/// When retention cleanup removed every journal row, including the change committed during the
/// disconnect, the journal can neither replay the change nor show that one was lost. The cache
/// must not keep its state from before the disconnect.
pub async fn test_notifier_resets_cache_after_journal_expiry(db: &PostgresDb) {
    let mut config = db.config.clone();
    config.schema = Some(format!("notifier_{}", Uuid::new_v4().simple()));
    let pool = make_pool(&config).await;
    let fixture = fixture(&config, &pool).await;
    let leaf_path = AccountPath::from_str(&format!("/{}/leaf", fixture.acme.segment)).unwrap();
    assert!(
        effective_state(&fixture.cache.resolve(&leaf_path).await.unwrap().unwrap())
            == AccountLifecycleState::Active
    );

    fixture.proxy.cut_and_hold().await;
    suspend(&fixture.tree, &fixture.acme).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let removed = fixture.journal.cleanup_old_events(0).await.unwrap();
    assert!(removed > 0);
    assert!(
        fixture
            .journal
            .get_latest_event_id()
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .journal
            .get_events_since(ChangeEventId(0))
            .await
            .unwrap()
            .is_empty()
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    fixture.proxy.release();

    assert!(
        becomes_suspended(
            &fixture.cache,
            fixture.leaf.account_id,
            Duration::from_secs(10)
        )
        .await,
        "the cache kept the state from before the disconnect"
    );
    let by_path = fixture.cache.resolve(&leaf_path).await.unwrap().unwrap();
    assert!(by_path.last().unwrap().account_id == fixture.leaf.account_id);
    assert!(effective_state(&by_path) == AccountLifecycleState::Suspended);
}

async fn record_unrelated_event(db: &PostgresDb) {
    db.pool
        .with_tx_err("notifier_reconnect_test", "record_event", |tx| {
            async move {
                DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(
                    tx,
                    &NewRegistryChangeEvent::account_tokens_invalidated(Uuid::new_v4()),
                )
                .await
            }
            .boxed()
        })
        .await
        .unwrap();
}
