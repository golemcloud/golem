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

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_common::config::{DbPostgresConfig, DbSqliteConfig};
use golem_common::model::ShardId;
use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
use golem_service_base::repo::{Blob, SqlDateTime};
use golem_shard_manager::config::EtcdConfig;
use golem_shard_manager::{
    DbRoutingTablePersistence, EtcdRoutingTablePersistence, ExecutorAddr, ExecutorId,
    ExternalRevision, LeaderFence, NO_REVISION, RoutingTablePersistence, STATE_KEY,
    ShardAssignmentEntry, ShardEpoch, ShardLeaseRevision, ShardLeaseState, ShardManagerError,
};
use golem_test_framework::components::etcd::docker_etcd::DockerEtcd;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep};
use url::Url;
use uuid::Uuid;

inherit_test_dep!(Arc<DockerEtcd>);

/// One `executor_leases` row, as a person reading the table would see it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LeaseRow {
    executor_id: Uuid,
    ip: IpAddr,
    port: i32,
    granted_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    pod_name: Option<String>,
}

/// The shape `SELECT executor_id, ip, port, granted_at, expires_at, pod_name` decodes into.
type RawLeaseRow = (
    Uuid,
    Blob<IpAddr>,
    i32,
    SqlDateTime,
    SqlDateTime,
    Option<String>,
);

/// What the local-mode mirror tables hold, normalized for comparison.
#[derive(Debug, PartialEq, Eq)]
struct MirrorSnapshot {
    leases: Vec<LeaseRow>,
    /// `(shard_id, executor_id, epoch)` per `shard_assignments` row, sorted.
    assignments: Vec<(i32, Uuid, i64)>,
}

impl MirrorSnapshot {
    fn of(shard_state: &ShardLeaseState) -> Self {
        let leases = shard_state
            .executor_leases
            .iter()
            .map(|(id, lease)| LeaseRow {
                executor_id: id.0,
                ip: lease.addr.ip,
                port: i32::from(lease.addr.port),
                granted_at: lease.granted_at,
                expires_at: lease.expires_at,
                pod_name: lease.pod_name.clone(),
            })
            .collect();
        let assignments = shard_state
            .shard_assignments
            .iter()
            .map(|(shard_id, entry)| {
                (
                    i32::try_from(shard_id.value()).unwrap(),
                    entry.executor_id.0,
                    i64::try_from(entry.epoch.0).unwrap(),
                )
            })
            .collect();
        Self::sorted(leases, assignments)
    }

    fn from_rows(leases: Vec<RawLeaseRow>, assignments: Vec<(i32, Uuid, i64)>) -> Self {
        let leases = leases
            .into_iter()
            .map(
                |(executor_id, ip, port, granted_at, expires_at, pod_name)| LeaseRow {
                    executor_id,
                    ip: ip.into_value(),
                    port,
                    granted_at: granted_at.into_utc(),
                    expires_at: expires_at.into_utc(),
                    pod_name,
                },
            )
            .collect();
        Self::sorted(leases, assignments)
    }

    fn empty() -> Self {
        Self::sorted(Vec::new(), Vec::new())
    }

    fn sorted(mut leases: Vec<LeaseRow>, mut assignments: Vec<(i32, Uuid, i64)>) -> Self {
        leases.sort();
        assignments.sort();
        Self {
            leases,
            assignments,
        }
    }
}

const LEASE_TTL: Duration = Duration::from_secs(60);
const NUMBER_OF_SHARDS: usize = 16;

/// A place where shard lease state can be stored.
///
/// Hands out any number of independent clients over the *same* underlying store, which is what
/// the compare-and-swap tests need: two clients must see each other's writes.
#[async_trait]
trait PersistenceStore: std::fmt::Debug + Send + Sync {
    async fn connect(&self) -> Arc<dyn RoutingTablePersistence>;

    /// Writes something unrelated to the shard state on the same backend, so a test can check
    /// that activity elsewhere on the backend does not move this store's revision.
    async fn unrelated_write(&self);

    /// Whether this backend has the local-mode mirror tables at all.
    fn has_mirror(&self) -> bool;

    /// The local-mode mirror tables, or `None` for a backend that has none.
    async fn mirror_snapshot(&self) -> Option<MirrorSnapshot>;
}

/// Creates isolated stores: two stores never see each other's data.
#[async_trait]
trait GetRoutingTablePersistence: std::fmt::Debug + Send + Sync {
    async fn new_store(&self) -> Arc<dyn PersistenceStore>;

    /// For the tests that only need a single client over a fresh store.
    async fn get_persistence(&self) -> Arc<dyn RoutingTablePersistence> {
        self.new_store().await.connect().await
    }
}

// -- postgres: isolated by a fresh database per store ------------------------------------------

struct PostgresRoutingTablePersistence {
    postgres: DockerPostgresRdb,
}

impl std::fmt::Debug for PostgresRoutingTablePersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PostgresRoutingTablePersistence")
    }
}

#[derive(Debug)]
struct PostgresStore {
    config: DbPostgresConfig,
}

#[async_trait]
impl PersistenceStore for PostgresStore {
    async fn connect(&self) -> Arc<dyn RoutingTablePersistence> {
        let pool = golem_service_base::db::postgres::PostgresPool::configured(&self.config)
            .await
            .expect("Cannot create postgres pool");

        Arc::new(DbRoutingTablePersistence::new(pool, NUMBER_OF_SHARDS))
    }

    async fn unrelated_write(&self) {
        use sqlx::Connection;

        let mut conn = sqlx::PgConnection::connect_with(&self.config.connect_options())
            .await
            .expect("Cannot connect to postgres");
        sqlx::query("CREATE TABLE IF NOT EXISTS unrelated_writes (id INTEGER)")
            .execute(&mut conn)
            .await
            .expect("Cannot create the unrelated table");
        sqlx::query("INSERT INTO unrelated_writes (id) VALUES (1)")
            .execute(&mut conn)
            .await
            .expect("Cannot perform the unrelated write");
    }

    fn has_mirror(&self) -> bool {
        true
    }

    async fn mirror_snapshot(&self) -> Option<MirrorSnapshot> {
        use sqlx::Connection;

        let mut conn = sqlx::PgConnection::connect_with(&self.config.connect_options())
            .await
            .expect("Cannot connect to the database");
        // One transaction, so both tables are read from the same committed state.
        let mut tx = conn.begin().await.expect("Cannot begin a transaction");
        let leases: Vec<RawLeaseRow> = sqlx::query_as(
            "SELECT executor_id, ip, port, granted_at, expires_at, pod_name FROM executor_leases",
        )
        .fetch_all(&mut *tx)
        .await
        .expect("Cannot read executor_leases");
        let assignments =
            sqlx::query_as("SELECT shard_id, executor_id, epoch FROM shard_assignments")
                .fetch_all(&mut *tx)
                .await
                .expect("Cannot read shard_assignments");
        tx.commit()
            .await
            .expect("Cannot commit the read transaction");
        Some(MirrorSnapshot::from_rows(leases, assignments))
    }
}

#[async_trait]
impl GetRoutingTablePersistence for PostgresRoutingTablePersistence {
    async fn new_store(&self) -> Arc<dyn PersistenceStore> {
        let db_name = format!("shard_{}", Uuid::new_v4().simple());

        let admin_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.postgres.public_connection_string())
            .await
            .expect("Cannot create postgres admin pool");

        sqlx::query(&format!("CREATE DATABASE \"{db_name}\";"))
            .execute(&admin_pool)
            .await
            .expect("Cannot create postgres test database");

        let config = DbPostgresConfig {
            host: "localhost".to_string(),
            database: db_name,
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            port: Url::parse(&self.postgres.public_connection_string())
                .expect("Invalid postgres connection string")
                .port()
                .expect("Postgres connection string missing port"),
            max_connections: 10,
            schema: None,
        };

        let migrations = IncludedMigrationsDir::new(&golem_shard_manager::DB_MIGRATIONS);

        golem_service_base::db::postgres::migrate(&config, migrations.postgres_migrations())
            .await
            .expect("Cannot apply postgres migrations");

        Arc::new(PostgresStore { config })
    }
}

// -- sqlite: isolated by a fresh file per store ------------------------------------------------

struct SqliteRoutingTablePersistence {
    temp_dir: TempDir,
}

impl std::fmt::Debug for SqliteRoutingTablePersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SqliteRoutingTablePersistence")
    }
}

#[derive(Debug)]
struct SqliteStore {
    config: DbSqliteConfig,
}

#[async_trait]
impl PersistenceStore for SqliteStore {
    async fn connect(&self) -> Arc<dyn RoutingTablePersistence> {
        let pool = golem_service_base::db::sqlite::SqlitePool::configured(&self.config)
            .await
            .expect("Cannot create sqlite pool");

        Arc::new(DbRoutingTablePersistence::new(pool, NUMBER_OF_SHARDS))
    }

    async fn unrelated_write(&self) {
        use sqlx::Connection;

        let mut conn = sqlx::SqliteConnection::connect_with(&self.config.connect_options())
            .await
            .expect("Cannot connect to sqlite");
        sqlx::query("CREATE TABLE IF NOT EXISTS unrelated_writes (id INTEGER)")
            .execute(&mut conn)
            .await
            .expect("Cannot create the unrelated table");
        sqlx::query("INSERT INTO unrelated_writes (id) VALUES (1)")
            .execute(&mut conn)
            .await
            .expect("Cannot perform the unrelated write");
    }

    fn has_mirror(&self) -> bool {
        true
    }

    async fn mirror_snapshot(&self) -> Option<MirrorSnapshot> {
        use sqlx::Connection;

        let mut conn = sqlx::SqliteConnection::connect_with(&self.config.connect_options())
            .await
            .expect("Cannot connect to the database");
        // One transaction, so both tables are read from the same committed state.
        let mut tx = conn.begin().await.expect("Cannot begin a transaction");
        let leases: Vec<RawLeaseRow> = sqlx::query_as(
            "SELECT executor_id, ip, port, granted_at, expires_at, pod_name FROM executor_leases",
        )
        .fetch_all(&mut *tx)
        .await
        .expect("Cannot read executor_leases");
        let assignments =
            sqlx::query_as("SELECT shard_id, executor_id, epoch FROM shard_assignments")
                .fetch_all(&mut *tx)
                .await
                .expect("Cannot read shard_assignments");
        tx.commit()
            .await
            .expect("Cannot commit the read transaction");
        Some(MirrorSnapshot::from_rows(leases, assignments))
    }
}

#[async_trait]
impl GetRoutingTablePersistence for SqliteRoutingTablePersistence {
    async fn new_store(&self) -> Arc<dyn PersistenceStore> {
        let database_file = self
            .temp_dir
            .path()
            .join(format!("shard_{}", Uuid::new_v4().simple()))
            .to_str()
            .expect("tempfile path was not valid unicode")
            .to_string();

        let config = DbSqliteConfig {
            database: database_file,
            max_connections: 10,
            foreign_keys: true,
        };

        let migrations = IncludedMigrationsDir::new(&golem_shard_manager::DB_MIGRATIONS);

        golem_service_base::db::sqlite::migrate(&config, migrations.sqlite_migrations())
            .await
            .expect("Cannot apply sqlite migrations");

        Arc::new(SqliteStore { config })
    }
}

// -- etcd: isolated by one server per test worker, plus a wipe per store -----------------------

struct EtcdRoutingTablePersistenceFactory {
    etcd: Arc<DockerEtcd>,
}

impl std::fmt::Debug for EtcdRoutingTablePersistenceFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EtcdRoutingTablePersistenceFactory")
    }
}

#[derive(Debug)]
struct EtcdStore {
    config: EtcdConfig,
}

impl EtcdStore {
    async fn kv(&self) -> etcd_client::KvClient {
        etcd_client::Client::connect(&self.config.endpoints, None)
            .await
            .expect("Cannot connect to etcd")
            .kv_client()
    }

    /// A stand-in for a won campaign's leader key, returned with the fence over it.
    ///
    /// The key is real and the fence carries its real creation revision: `for_test(key, 0)` would
    /// compare `create_revision == 0` - "this key must not exist" - inverting the fence.
    async fn mint_leader_key(&self) -> (String, LeaderFence) {
        let key = format!("/golem/test/leader/{}", Uuid::new_v4());
        let mut kv = self.kv().await;
        kv.put(key.clone(), "test-leader", None)
            .await
            .expect("Cannot put the sentinel leader key");
        let created = kv
            .get(key.clone(), None)
            .await
            .expect("Cannot read the sentinel leader key back")
            .kvs()
            .first()
            .expect("The sentinel leader key should exist immediately after being written")
            .create_revision();

        (key.clone(), LeaderFence::for_test(key, created))
    }

    async fn persistence_with(&self, fence: LeaderFence) -> Arc<dyn RoutingTablePersistence> {
        Arc::new(
            EtcdRoutingTablePersistence::new(&self.config, NUMBER_OF_SHARDS, fence)
                .await
                .expect("Cannot connect to etcd"),
        )
    }
}

#[async_trait]
impl PersistenceStore for EtcdStore {
    async fn connect(&self) -> Arc<dyn RoutingTablePersistence> {
        let (_, fence) = self.mint_leader_key().await;
        self.persistence_with(fence).await
    }

    async fn unrelated_write(&self) {
        // Any write advances etcd's cluster-wide revision; the state key's mod_revision must not
        // follow it.
        self.kv()
            .await
            .put("/golem/test/unrelated", "x", None)
            .await
            .expect("Cannot perform the unrelated write");
    }

    fn has_mirror(&self) -> bool {
        false
    }

    async fn mirror_snapshot(&self) -> Option<MirrorSnapshot> {
        // Mirror tables are a local-mode feature; distributed mode holds only the blob.
        None
    }
}

impl EtcdRoutingTablePersistenceFactory {
    /// The concrete store, for the etcd-only tests that tamper with the leader key.
    async fn new_etcd_store(&self) -> EtcdStore {
        // The state key is fixed, so stores on one etcd server cannot be isolated from each other
        // the way postgres stores are by database. Instead the server is per test worker - tests
        // on a worker run one at a time - and every new store starts by wiping the key.
        let store = EtcdStore {
            config: EtcdConfig {
                endpoints: vec![self.etcd.client_url()],
                ..EtcdConfig::default()
            },
        };
        store
            .kv()
            .await
            .delete(STATE_KEY, None)
            .await
            .expect("Cannot wipe the etcd state key");
        store
    }
}

#[async_trait]
impl GetRoutingTablePersistence for EtcdRoutingTablePersistenceFactory {
    async fn new_store(&self) -> Arc<dyn PersistenceStore> {
        Arc::new(self.new_etcd_store().await)
    }
}

/// A wiped etcd store, the sentinel leader key its persistence is fenced on, and that persistence.
async fn fenced_etcd_persistence(
    etcd: &Arc<DockerEtcd>,
) -> (EtcdStore, String, Arc<dyn RoutingTablePersistence>) {
    let store = EtcdRoutingTablePersistenceFactory { etcd: etcd.clone() }
        .new_etcd_store()
        .await;
    let (leader_key, fence) = store.mint_leader_key().await;
    let persistence = store.persistence_with(fence).await;
    (store, leader_key, persistence)
}

#[test_dep(scope = Shared, tagged_as = "sqlite")]
async fn sqlite_persistence() -> Arc<dyn GetRoutingTablePersistence> {
    let temp_dir = TempDir::new().expect("Cannot create temp dir");
    Arc::new(SqliteRoutingTablePersistence { temp_dir })
}

#[test_dep(scope = Shared, tagged_as = "postgres")]
async fn postgres_persistence() -> Arc<dyn GetRoutingTablePersistence> {
    let unique_network_id = Uuid::new_v4().to_string();
    let postgres = DockerPostgresRdb::new(&unique_network_id, false).await;
    Arc::new(PostgresRoutingTablePersistence { postgres })
}

#[test_dep(scope = PerWorker, tagged_as = "etcd")]
async fn etcd_persistence(etcd: &Arc<DockerEtcd>) -> Arc<dyn GetRoutingTablePersistence> {
    Arc::new(EtcdRoutingTablePersistenceFactory { etcd: etcd.clone() })
}

define_matrix_dimension!(persistence: Arc<dyn GetRoutingTablePersistence> -> "sqlite", "postgres", "etcd");

#[test]
#[tracing::instrument]
async fn read_returns_default_when_empty(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let persistence = persistence.get_persistence().await;
    let (shard_state, revision) = persistence
        .read()
        .await
        .expect("Reading default shard lease state should succeed");

    assert_eq!(revision, NO_REVISION);
    assert_eq!(shard_state.number_of_shards, NUMBER_OF_SHARDS);
    assert_eq!(shard_state.revision, ShardLeaseRevision::INITIAL);
    assert!(shard_state.shard_assignments.is_empty());
    assert!(shard_state.executor_leases.is_empty());
    assert!(shard_state.pending_rebalance.is_empty());
}

#[test]
#[tracing::instrument]
async fn write_then_read_roundtrip(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let persistence = persistence.get_persistence().await;
    let expected = sample_shard_state(NUMBER_OF_SHARDS);

    let written_revision = persistence
        .write(&expected, NO_REVISION)
        .await
        .expect("Writing routing table should succeed");
    assert!(written_revision > NO_REVISION);

    let (actual, read_revision) = persistence
        .read()
        .await
        .expect("Reading persisted routing table should succeed");

    assert_eq!(actual, expected);
    assert_eq!(read_revision, written_revision);
}

#[test]
#[tracing::instrument]
async fn sequential_writes_advance_the_revision(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let persistence = persistence.get_persistence().await;
    let first = sample_shard_state(NUMBER_OF_SHARDS);
    let second = replacement_shard_state(NUMBER_OF_SHARDS);

    let first_revision = persistence
        .write(&first, NO_REVISION)
        .await
        .expect("Writing first routing table should succeed");
    let second_revision = persistence
        .write(&second, first_revision)
        .await
        .expect("Writing second routing table should succeed");

    // Strictly greater, never `first_revision + 1`: etcd's revision is a cluster-global counter.
    assert!(second_revision > first_revision);

    let (actual, revision) = persistence
        .read()
        .await
        .expect("Reading persisted routing table should succeed");

    assert_eq!(actual, second);
    assert_eq!(revision, second_revision);
}

#[test]
#[tracing::instrument]
async fn writing_the_same_state_twice_still_advances_the_revision(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    // Guards against ever deriving the storage revision from the in-blob ShardLeaseRevision: a
    // write that does not change the state must still move the compare-and-swap token, otherwise
    // two concurrent such writes would both succeed.
    let persistence = persistence.get_persistence().await;
    let shard_state = sample_shard_state(NUMBER_OF_SHARDS);

    let first = persistence
        .write(&shard_state, NO_REVISION)
        .await
        .expect("Writing routing table should succeed");
    let second = persistence
        .write(&shard_state, first)
        .await
        .expect("Rewriting the same routing table should succeed");

    assert!(second > first);
}

#[test]
#[tracing::instrument]
async fn mirror_tables_follow_the_persisted_state(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let ours = store.connect().await;

    let first = sample_shard_state(NUMBER_OF_SHARDS);
    let first_revision = ours
        .write(&first, NO_REVISION)
        .await
        .expect("Writing the first routing table should succeed");

    let snapshot = store.mirror_snapshot().await;
    assert_eq!(
        snapshot.is_some(),
        store.has_mirror(),
        "a backend must report a mirror exactly when it has one"
    );
    let Some(snapshot) = snapshot else {
        // Distributed mode has no mirror tables; nothing to check on this backend.
        return;
    };
    assert_eq!(snapshot, MirrorSnapshot::of(&first));

    // A rewrite replaces the mirror wholesale: rows of executors that are gone must not linger.
    let second = replacement_shard_state(NUMBER_OF_SHARDS);
    let second_revision = ours
        .write(&second, first_revision)
        .await
        .expect("Writing the second routing table should succeed");
    assert_eq!(
        store.mirror_snapshot().await.unwrap(),
        MirrorSnapshot::of(&second)
    );

    // The mirror rides the blob's compare-and-swap: a rejected write leaves it untouched.
    let result = ours.write(&first, first_revision).await;
    assert!(
        matches!(result, Err(ShardManagerError::ConcurrentModification)),
        "expected ConcurrentModification, got {result:?}"
    );
    assert_eq!(
        store.mirror_snapshot().await.unwrap(),
        MirrorSnapshot::of(&second)
    );
    let (_, revision) = ours
        .read()
        .await
        .expect("Reading the routing table should succeed");
    assert_eq!(revision, second_revision);

    // ...and empties out with the last lease.
    let mut emptied = second.clone();
    let _ = emptied.housekeep(granted_at() + chrono::Duration::from_std(LEASE_TTL).unwrap());
    emptied
        .bump_revision()
        .expect("revision bump should succeed");
    assert!(emptied.executor_leases.is_empty());
    assert!(emptied.shard_assignments.is_empty());
    ours.write(&emptied, second_revision)
        .await
        .expect("Writing the emptied routing table should succeed");
    assert_eq!(
        store.mirror_snapshot().await.unwrap(),
        MirrorSnapshot::empty()
    );
}

#[test]
#[tracing::instrument]
async fn a_full_size_routing_table_roundtrips_and_is_mirrored(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    // The production default is 1024 shards - more than one multi-row INSERT chunk of the mirror
    // tables, so this is the path every real local-mode deployment takes on every write.
    const SHARDS: usize = 1024;
    const EXECUTORS: usize = 4;
    let shard_ids: Vec<Vec<i64>> = (0..EXECUTORS)
        .map(|executor| {
            (0..SHARDS as i64)
                .filter(|shard| *shard as usize % EXECUTORS == executor)
                .collect()
        })
        .collect();
    let executors: Vec<(ExecutorId, ExecutorAddr, Option<&str>, &[i64])> = (0..EXECUTORS)
        .map(|executor| {
            (
                self::executor(executor as u128 + 1),
                addr(executor as u8 + 1, 9010 + executor as u16),
                None,
                shard_ids[executor].as_slice(),
            )
        })
        .collect();
    let shard_state = shard_state_with_executors(SHARDS, &executors);
    assert_eq!(shard_state.shard_assignments.len(), SHARDS);

    let store = persistence.new_store().await;
    let ours = store.connect().await;

    let revision = ours
        .write(&shard_state, NO_REVISION)
        .await
        .expect("Writing a full-size routing table should succeed");
    let (actual, actual_revision) = ours
        .read()
        .await
        .expect("Reading a full-size routing table should succeed");
    assert_eq!(actual, shard_state);
    assert_eq!(actual_revision, revision);

    let snapshot = store.mirror_snapshot().await;
    assert_eq!(
        snapshot.is_some(),
        store.has_mirror(),
        "a backend must report a mirror exactly when it has one"
    );
    // The only coverage of writes spanning more than one MIRROR_INSERT_CHUNK_SIZE chunk, which
    // every production-sized write does, so it must not become skippable by accident.
    if let Some(snapshot) = snapshot {
        assert_eq!(snapshot.assignments.len(), SHARDS);
        assert_eq!(snapshot, MirrorSnapshot::of(&shard_state));
    }
}

#[test]
#[tracing::instrument]
async fn a_state_that_violates_invariants_is_refused_before_it_is_stored(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    // Every backend refuses identically and before any I/O - not by whichever constraint a
    // backend happens to enforce (the SQL mirror's foreign key, which SQLite only checks with
    // foreign_keys = true), and not by poisoning the store for every later read.
    let store = persistence.new_store().await;
    let ours = store.connect().await;

    let good = sample_shard_state(NUMBER_OF_SHARDS);
    let revision = ours
        .write(&good, NO_REVISION)
        .await
        .expect("Writing the routing table should succeed");

    let mut bad = good.clone();
    // shard 5 is unassigned in the fixture; executor 42 holds no lease
    bad.shard_assignments.insert(
        ShardId::new(5),
        ShardAssignmentEntry {
            executor_id: executor(42),
            epoch: ShardEpoch::initial(),
        },
    );
    let result = ours.write(&bad, revision).await;
    assert!(
        matches!(result, Err(ShardManagerError::Internal(_))),
        "expected Internal, got {result:?}"
    );

    let (actual, actual_revision) = ours
        .read()
        .await
        .expect("Reading the routing table should succeed");
    assert_eq!(actual, good);
    assert_eq!(actual_revision, revision);
    if let Some(snapshot) = store.mirror_snapshot().await {
        assert_eq!(snapshot, MirrorSnapshot::of(&good));
    }
}

#[test]
#[tracing::instrument]
async fn housekeep_expiry_reclamation_roundtrips(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    // The state shape `housekeep` produces - leases gone, their shards moved to
    // `pending_rebalance`, `shard_epochs` retained as the high-water mark - has to survive a
    // round-trip on every backend.
    let persistence = persistence.get_persistence().await;
    let mut shard_state = sample_shard_state(NUMBER_OF_SHARDS);
    let before = shard_state.executor_count();

    let expired =
        shard_state.housekeep(granted_at() + chrono::Duration::from_std(LEASE_TTL).unwrap());
    assert_eq!(
        expired.len(),
        before,
        "every lease in the fixture should have expired at granted_at + ttl"
    );
    assert!(shard_state.executor_leases.is_empty());
    assert!(!shard_state.pending_rebalance.is_empty());
    assert!(shard_state.shard_assignments.is_empty());
    shard_state
        .bump_revision()
        .expect("revision bump should succeed");

    let revision = persistence
        .write(&shard_state, NO_REVISION)
        .await
        .expect("Writing the reclaimed state should succeed");

    let (actual, actual_revision) = persistence
        .read()
        .await
        .expect("Reading the reclaimed state should succeed");

    assert_eq!(actual, shard_state);
    assert_eq!(actual_revision, revision);
    // The epoch high-water mark must outlive the leases, or a reassigned shard could reuse an
    // epoch a fenced zombie still holds.
    assert!(!actual.shard_epochs.is_empty());
}

#[test]
#[tracing::instrument]
async fn two_clients_over_the_same_store_see_each_others_writes(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    // A sanity check on the fixture itself: if `connect` handed out isolated stores, the
    // compare-and-swap tests below would pass vacuously.
    let store = persistence.new_store().await;
    let first = store.connect().await;
    let second = store.connect().await;

    let expected = sample_shard_state(NUMBER_OF_SHARDS);
    let revision = first
        .write(&expected, NO_REVISION)
        .await
        .expect("Writing routing table should succeed");

    let (actual, actual_revision) = second
        .read()
        .await
        .expect("Reading persisted routing table should succeed");

    assert_eq!(actual, expected);
    assert_eq!(actual_revision, revision);
}

#[test]
#[tracing::instrument]
async fn write_with_no_revision_when_already_present_is_rejected(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    // A cold-started rival that believes the store is empty must not overwrite live state.
    let store = persistence.new_store().await;
    let first = store.connect().await;
    let second = store.connect().await;

    first
        .write(&sample_shard_state(NUMBER_OF_SHARDS), NO_REVISION)
        .await
        .expect("Writing the initial routing table should succeed");

    let result = second
        .write(&replacement_shard_state(NUMBER_OF_SHARDS), NO_REVISION)
        .await;

    assert!(
        matches!(result, Err(ShardManagerError::ConcurrentModification)),
        "expected ConcurrentModification, got {result:?}"
    );
}

#[test]
#[tracing::instrument]
async fn write_with_a_superseded_revision_is_rejected(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let store = persistence.new_store().await;
    let first = store.connect().await;
    let second = store.connect().await;

    let initial = sample_shard_state(NUMBER_OF_SHARDS);
    let winner = replacement_shard_state(NUMBER_OF_SHARDS);

    let stale_revision = first
        .write(&initial, NO_REVISION)
        .await
        .expect("Writing the initial routing table should succeed");
    let winning_revision = first
        .write(&winner, stale_revision)
        .await
        .expect("Writing the second routing table should succeed");

    let result = second.write(&initial, stale_revision).await;
    assert!(
        matches!(result, Err(ShardManagerError::ConcurrentModification)),
        "expected ConcurrentModification, got {result:?}"
    );

    // The loser's payload must never become visible.
    let (actual, revision) = second
        .read()
        .await
        .expect("Reading persisted routing table should succeed");
    assert_eq!(actual, winner);
    assert_eq!(revision, winning_revision);
}

#[test]
#[tracing::instrument]
async fn write_with_a_revision_when_nothing_is_stored_is_rejected(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    // A revision against absent state must fail rather than resurrect it. This is the case a
    // guarded upsert gets wrong on SQL, because its INSERT branch is unguarded.
    let persistence = persistence.get_persistence().await;

    let result = persistence
        .write(&sample_shard_state(NUMBER_OF_SHARDS), 12345)
        .await;

    assert!(
        matches!(result, Err(ShardManagerError::ConcurrentModification)),
        "expected ConcurrentModification, got {result:?}"
    );

    let (_, revision) = persistence
        .read()
        .await
        .expect("Reading shard lease state should succeed");
    assert_eq!(revision, NO_REVISION);
}

#[test]
#[tracing::instrument]
async fn conflict_does_not_advance_the_stored_revision(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    let persistence = persistence.get_persistence().await;
    let stored = sample_shard_state(NUMBER_OF_SHARDS);

    let revision = persistence
        .write(&stored, NO_REVISION)
        .await
        .expect("Writing routing table should succeed");

    let result = persistence
        .write(&replacement_shard_state(NUMBER_OF_SHARDS), NO_REVISION)
        .await;
    assert!(
        matches!(result, Err(ShardManagerError::ConcurrentModification)),
        "expected ConcurrentModification, got {result:?}"
    );

    let (actual, actual_revision) = persistence
        .read()
        .await
        .expect("Reading persisted routing table should succeed");
    assert_eq!(actual, stored);
    assert_eq!(actual_revision, revision);
}

#[test]
#[tracing::instrument]
async fn an_unrelated_write_on_the_backend_does_not_move_our_revision(
    #[dimension(persistence)] persistence: &Arc<dyn GetRoutingTablePersistence>,
) {
    // The revision must belong to the key/row, not to the backend as a whole. This is the one
    // test that catches returning etcd's cluster-global header revision from `read`.
    let store = persistence.new_store().await;
    let ours = store.connect().await;

    let revision = ours
        .write(&sample_shard_state(NUMBER_OF_SHARDS), NO_REVISION)
        .await
        .expect("Writing our routing table should succeed");

    store.unrelated_write().await;

    let (_, our_revision): (ShardLeaseState, ExternalRevision) = ours
        .read()
        .await
        .expect("Reading our routing table should succeed");
    assert_eq!(our_revision, revision);
}

// -- etcd only: the leadership fence ------------------------------------------------------------
//
// Outside the three-backend matrix because the SQL backends have no leader key to fence on.

#[test]
#[tracing::instrument(skip_all)]
async fn a_write_after_the_leader_key_is_deleted_is_rejected_as_leadership_lost(
    etcd: &Arc<DockerEtcd>,
) {
    let (store, leader_key, persistence) = fenced_etcd_persistence(etcd).await;

    let stored = sample_shard_state(NUMBER_OF_SHARDS);
    let revision = persistence
        .write(&stored, NO_REVISION)
        .await
        .expect("Writing the initial routing table should succeed");

    // Deleting the key is what losing leadership looks like: the lease expires and it goes away.
    store
        .kv()
        .await
        .delete(leader_key, None)
        .await
        .expect("Cannot delete the sentinel leader key");

    // A different payload, so the read-back below distinguishes refusal from a same-bytes write.
    let result = persistence
        .write(&replacement_shard_state(NUMBER_OF_SHARDS), revision)
        .await;
    assert!(
        matches!(result, Err(ShardManagerError::LeadershipLost { .. })),
        "A demoted leader's write must be refused as LeadershipLost - without the fence compare it \
         would pass on the state revision alone and overwrite the new leader's work. Got {result:?}"
    );

    let (actual, actual_revision) = persistence
        .read()
        .await
        .expect("Reading persisted routing table should succeed");
    assert_eq!(
        actual, stored,
        "The refused write must not have replaced the stored state"
    );
    assert_eq!(
        actual_revision, revision,
        "The refused write must not have advanced the stored revision"
    );
}

#[test]
#[tracing::instrument(skip_all)]
async fn a_write_after_the_leader_key_is_recreated_is_rejected_as_leadership_lost(
    etcd: &Arc<DockerEtcd>,
) {
    let (store, leader_key, persistence) = fenced_etcd_persistence(etcd).await;

    let stored = sample_shard_state(NUMBER_OF_SHARDS);
    let revision = persistence
        .write(&stored, NO_REVISION)
        .await
        .expect("Writing the initial routing table should succeed");

    // Models the failover: this replica's key goes away and another campaign puts one back at the
    // same path, so only the creation revision tells the two leaders apart.
    let mut kv = store.kv().await;
    kv.delete(leader_key.clone(), None)
        .await
        .expect("Cannot delete the sentinel leader key");
    kv.put(leader_key, "a-later-leader", None)
        .await
        .expect("Cannot recreate the sentinel leader key");

    let result = persistence
        .write(&replacement_shard_state(NUMBER_OF_SHARDS), revision)
        .await;
    assert!(
        matches!(result, Err(ShardManagerError::LeadershipLost { .. })),
        "The fence must compare the creation revision the campaign won, not merely that some key \
         exists at that path. Got {result:?}"
    );

    let (actual, actual_revision) = persistence
        .read()
        .await
        .expect("Reading persisted routing table should succeed");
    assert_eq!(
        actual, stored,
        "The refused write must not have replaced the stored state"
    );
    assert_eq!(
        actual_revision, revision,
        "The refused write must not have advanced the stored revision"
    );
}

fn granted_at() -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
}

fn executor(idx: u128) -> ExecutorId {
    ExecutorId(Uuid::from_u128(idx))
}

fn addr(last_octet: u8, port: u16) -> ExecutorAddr {
    ExecutorAddr {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, last_octet)),
        port,
    }
}

fn shard_state_with_executors(
    number_of_shards: usize,
    executors: &[(ExecutorId, ExecutorAddr, Option<&str>, &[i64])],
) -> ShardLeaseState {
    let mut shard_state = ShardLeaseState::new(number_of_shards);
    for (executor_id, addr, pod_name, shard_ids) in executors {
        shard_state.add_executor(
            *executor_id,
            *addr,
            pod_name.map(str::to_string),
            granted_at(),
            LEASE_TTL,
        );
        for shard_id in *shard_ids {
            shard_state.assign_shard(*executor_id, ShardId::new(*shard_id));
        }
    }
    shard_state
        .bump_revision()
        .expect("revision bump should succeed");
    shard_state
}

fn sample_shard_state(number_of_shards: usize) -> ShardLeaseState {
    let mut shard_state = shard_state_with_executors(
        number_of_shards,
        &[
            (
                executor(1),
                addr(1, 9010),
                Some("worker-executor-0"),
                &[0, 1, 2],
            ),
            (executor(2), addr(2, 9011), None, &[3, 4]),
        ],
    );
    // make sure a non-initial epoch and an orphaned shard survive the roundtrip too
    shard_state.assign_shard(executor(2), ShardId::new(0));
    shard_state.add_executor(executor(3), addr(3, 9012), None, granted_at(), LEASE_TTL);
    shard_state.assign_shard(executor(3), ShardId::new(9));
    shard_state.remove_executor(executor(3));
    shard_state
}

fn replacement_shard_state(number_of_shards: usize) -> ShardLeaseState {
    shard_state_with_executors(
        number_of_shards,
        &[(executor(3), addr(3, 9012), None, &[5, 6, 7])],
    )
}
