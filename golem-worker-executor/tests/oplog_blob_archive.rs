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
use aws_config::BehaviorVersion;
use aws_config::meta::region::RegionProviderChain;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::Credentials;
use golem_common::config::RedisConfig;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{LogLevel, OplogEntry, OplogIndex};
use golem_common::model::{AgentId, OwnedAgentId, RetryConfig, ScanCursor, ShardEpoch};
use golem_common::redis::RedisPool;
use golem_service_base::config::{S3BlobStorageConfig, S3BlobStorageCredentialsConfig};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, s3};
use golem_test_framework::components::s3_mock::{DockerS3Mock, S3Mock};
use golem_worker_executor::services::oplog::{BlobOplogArchiveService, OplogArchiveService};
use golem_worker_executor::storage::indexed::memory::InMemoryIndexedStorage;
use golem_worker_executor::storage::indexed::redis::RedisIndexedStorage;
use golem_worker_executor_test_utils::WorkerExecutorTestDependencies;
use pretty_assertions::assert_eq;
use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

inherit_test_dep!(WorkerExecutorTestDependencies);

#[async_trait]
trait GetBlobStorage: Debug + Send + Sync {
    async fn get_blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync>;
}

struct InMemoryTest;

impl Debug for InMemoryTest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InMemoryTest")
    }
}

#[async_trait]
impl GetBlobStorage for InMemoryTest {
    async fn get_blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync> {
        Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
    }
}

#[test_dep(scope = PerWorker, tagged_as = "in_memory")]
fn in_memory() -> Arc<dyn GetBlobStorage + Send + Sync> {
    Arc::new(InMemoryTest)
}

/// Spins up a fresh S3Mock container per `get_blob_storage` call and keeps it
/// alive for the lifetime of this per-worker dependency, so the returned S3
/// blob storage remains usable for the whole test.
struct S3Test {
    s3_mock_instances: Mutex<Vec<DockerS3Mock>>,
}

impl Debug for S3Test {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "S3Test")
    }
}

#[async_trait]
impl GetBlobStorage for S3Test {
    async fn get_blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync> {
        let s3_mock = DockerS3Mock::new().await;

        let config = S3BlobStorageConfig {
            retries: Default::default(),
            region: "us-east-1".to_string(),
            object_prefix: String::new(),
            aws_endpoint_url: Some(s3_mock.endpoint()),
            aws_credentials: Some(S3BlobStorageCredentialsConfig::new(
                s3_mock.access_key_id(),
                s3_mock.secret_access_key(),
                "test",
            )),
            aws_path_style: Some(true),
            ..std::default::Default::default()
        };
        create_buckets(&s3_mock, &config).await;
        let storage = s3::S3BlobStorage::new(config).await;

        self.s3_mock_instances.lock().await.push(s3_mock);
        Arc::new(storage)
    }
}

async fn create_buckets(s3_mock: &dyn S3Mock, config: &S3BlobStorageConfig) {
    let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");
    let creds = Credentials::new(
        s3_mock.access_key_id(),
        s3_mock.secret_access_key(),
        None,
        None,
        "test",
    );
    let sdk_config = aws_config::defaults(BehaviorVersion::latest())
        .region(region_provider)
        .endpoint_url(s3_mock.endpoint())
        .credentials_provider(creds)
        .load()
        .await;

    let client = Client::from_conf(
        aws_sdk_s3::config::Builder::from(&sdk_config)
            .force_path_style(true)
            .build(),
    );
    for bucket in &config.compressed_oplog_buckets {
        client.create_bucket().bucket(bucket).send().await.unwrap();
    }
}

#[test_dep(scope = PerWorker, tagged_as = "s3")]
fn s3() -> Arc<dyn GetBlobStorage + Send + Sync> {
    Arc::new(S3Test {
        s3_mock_instances: Mutex::new(Vec::new()),
    })
}

define_matrix_dimension!(storage: Arc<dyn GetBlobStorage + Send + Sync> -> "in_memory", "s3");

async fn append_worker(
    service: &BlobOplogArchiveService,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
) {
    let archive = service.open(owned_agent_id, agent_mode, None).await;
    archive
        .append(&[(
            OplogIndex::INITIAL,
            OplogEntry::log(
                None,
                LogLevel::Debug,
                "test".to_string(),
                "test".to_string(),
                None,
            ),
        )])
        .await
        .unwrap();
}

async fn drain(
    service: &BlobOplogArchiveService,
    environment_id: &EnvironmentId,
    component_id: &ComponentId,
    modes: Option<AgentMode>,
    page_size: u64,
) -> Vec<OwnedAgentId> {
    let mut cursor = ScanCursor::default();
    let mut acc: Vec<OwnedAgentId> = Vec::new();
    loop {
        let (next_cursor, ids) = service
            .scan_for_component(environment_id, component_id, modes, cursor, page_size)
            .await
            .unwrap();
        assert!(ids.len() as u64 <= page_size);
        acc.extend(ids);
        if next_cursor.is_finished() {
            break;
        }
        cursor = next_cursor;
    }
    acc.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
    acc
}

/// The blob oplog archive holds the lowest-layer copies of ephemeral (and
/// archived durable) workers. This test verifies that `scan_for_component`
/// against the blob archive lists workers correctly and filters by agent mode,
/// running the same checks against both an in-memory backend and a real
/// S3-compatible (Adobe S3Mock) backend.
#[test]
async fn blob_archive_scan_for_component_filters_by_mode(
    #[dimension(storage)] storage: &Arc<dyn GetBlobStorage + Send + Sync>,
) {
    let blob_storage = storage.get_blob_storage().await;

    // `compressed_oplog_buckets[0]` ("oplog-archive-1") corresponds to level 0.
    let service = BlobOplogArchiveService::new(
        blob_storage,
        Arc::new(InMemoryIndexedStorage::new()),
        0,
        RetryConfig::default(),
    );

    let environment_id = EnvironmentId::new();
    let component_id = ComponentId::new();

    let make_owned = |name: String| {
        OwnedAgentId::new(
            environment_id,
            &AgentId {
                component_id,
                agent_id: name,
            },
        )
    };

    let mut ephemeral = Vec::new();
    for i in 0..5 {
        let owned = make_owned(format!("eph-{i}"));
        append_worker(&service, &owned, AgentMode::Ephemeral).await;
        ephemeral.push(owned);
    }
    let mut durable = Vec::new();
    for i in 0..3 {
        let owned = make_owned(format!("dur-{i}"));
        append_worker(&service, &owned, AgentMode::Durable).await;
        durable.push(owned);
    }

    let mut expected_ephemeral = ephemeral.clone();
    expected_ephemeral.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
    let mut expected_durable = durable.clone();
    expected_durable.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));
    let mut expected_both: Vec<OwnedAgentId> = ephemeral.into_iter().chain(durable).collect();
    expected_both.sort_by(|a, b| a.agent_id.agent_id.cmp(&b.agent_id.agent_id));

    assert_eq!(
        drain(
            &service,
            &environment_id,
            &component_id,
            Some(AgentMode::Ephemeral),
            100
        )
        .await,
        expected_ephemeral
    );
    assert_eq!(
        drain(
            &service,
            &environment_id,
            &component_id,
            Some(AgentMode::Durable),
            100
        )
        .await,
        expected_durable
    );
    assert_eq!(
        drain(&service, &environment_id, &component_id, None, 100).await,
        expected_both
    );
    assert_eq!(
        drain(&service, &environment_id, &component_id, None, 2).await,
        expected_both
    );

    // A component with no archived workers yields an empty scan.
    let empty_component_id = ComponentId::new();
    assert_eq!(
        drain(&service, &environment_id, &empty_component_id, None, 100).await,
        Vec::<OwnedAgentId>::new()
    );
}

/// A relay in front of Redis that loses the reply of the first manifest append: the append's
/// script runs, and the client's connection is closed before its reply is passed on. After that
/// the relay accepts no connection until `resume` is notified, so a test can act between the lost
/// reply and the client's reconnection.
struct LostAppendReplyRelay {
    port: u16,
    /// How many manifest appends were passed on to Redis.
    appends: Arc<AtomicUsize>,
    lost: Arc<Notify>,
    resume: Arc<Notify>,
    server: tokio::task::JoinHandle<()>,
}

impl LostAppendReplyRelay {
    async fn start(redis_host: String, redis_port: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let appends = Arc::new(AtomicUsize::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        let lost = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let server = tokio::spawn({
            let appends = appends.clone();
            let lost = lost.clone();
            let resume = resume.clone();
            async move {
                while let Ok((client, _)) = listener.accept().await {
                    if paused.swap(false, Ordering::SeqCst) {
                        resume.notified().await;
                    }
                    let Ok(redis) = TcpStream::connect((redis_host.as_str(), redis_port)).await
                    else {
                        continue;
                    };
                    tokio::spawn(Self::relay(
                        client,
                        redis,
                        appends.clone(),
                        paused.clone(),
                        lost.clone(),
                    ));
                }
            }
        });
        Self {
            port,
            appends,
            lost,
            resume,
            server,
        }
    }

    async fn relay(
        mut client: TcpStream,
        mut redis: TcpStream,
        appends: Arc<AtomicUsize>,
        paused: Arc<AtomicBool>,
        lost: Arc<Notify>,
    ) {
        let (mut from_client, mut to_client) = client.split();
        let (mut from_redis, mut to_redis) = redis.split();
        let mut request = vec![0; 64 * 1024];
        let mut reply = vec![0; 64 * 1024];
        let mut lose_reply = false;
        loop {
            tokio::select! {
                read = from_client.read(&mut request) => {
                    let Ok(n @ 1..) = read else { return };
                    // Of the scripts a manifest runs, only the append reads the stream's
                    // `last-generated-id`, and it names it once.
                    const APPEND_MARKER: &[u8] = b"last-generated-id";
                    if request[..n]
                        .windows(APPEND_MARKER.len())
                        .any(|window| window == APPEND_MARKER)
                        && appends.fetch_add(1, Ordering::SeqCst) == 0
                    {
                        lose_reply = true;
                    }
                    if to_redis.write_all(&request[..n]).await.is_err() {
                        return;
                    }
                }
                read = from_redis.read(&mut reply) => {
                    let Ok(n @ 1..) = read else { return };
                    if lose_reply {
                        paused.store(true, Ordering::SeqCst);
                        lost.notify_one();
                        return;
                    }
                    if to_client.write_all(&reply[..n]).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// A former owner's manifest append is stored in Redis and its reply is lost. A newer owner
/// records its epoch before the former owner's client is connected again. The chunk is listed, so
/// its object must stay and the new owner must read it. The append must reach Redis once: sent
/// again it would be refused, and the refusal would say nothing about the run that stored the
/// entry.
#[test]
async fn blob_chunk_listed_by_an_append_with_a_lost_reply_stays_readable(
    deps: &WorkerExecutorTestDependencies,
) {
    let redis = deps.redis.clone();
    redis.assert_valid();
    let relay = LostAppendReplyRelay::start(redis.public_host(), redis.public_port()).await;

    let key_prefix = Uuid::new_v4().to_string();
    let blob_storage: Arc<dyn BlobStorage + Send + Sync> = Arc::new(InMemoryBlobStorage::new());
    let blob_level = |host: String, port: u16| {
        let key_prefix = key_prefix.clone();
        let blob_storage = blob_storage.clone();
        async move {
            let pool = RedisPool::configured(&RedisConfig {
                host,
                port,
                database: 0,
                tracing: false,
                pool_size: 1,
                retries: Default::default(),
                key_prefix,
                username: None,
                password: None,
                tls: false,
            })
            .await
            .unwrap();
            BlobOplogArchiveService::new(
                blob_storage,
                Arc::new(RedisIndexedStorage::new(pool)),
                0,
                RetryConfig::default(),
            )
        }
    };
    let through_relay = blob_level("127.0.0.1".to_string(), relay.port).await;
    let direct = blob_level(redis.public_host(), redis.public_port()).await;

    let owned_agent_id = OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "lost-append-reply".to_string(),
        },
    );
    let log = || OplogEntry::log(None, LogLevel::Debug, "test".into(), "test".into(), None);
    let entries = vec![
        (OplogIndex::INITIAL, log()),
        (OplogIndex::from_u64(2), log()),
    ];

    let former_owner = through_relay
        .open(&owned_agent_id, AgentMode::Durable, Some(ShardEpoch(5)))
        .await;
    let appending = tokio::spawn({
        let entries = entries.clone();
        async move { former_owner.append(&entries).await }
    });

    tokio::time::timeout(Duration::from_secs(30), relay.lost.notified())
        .await
        .expect("the manifest append never reached Redis");
    let new_owner = direct
        .open(&owned_agent_id, AgentMode::Durable, Some(ShardEpoch(6)))
        .await;
    relay.resume.notify_one();
    let appended = tokio::time::timeout(Duration::from_secs(30), appending)
        .await
        .expect("the append did not return")
        .unwrap();

    assert_eq!(
        new_owner
            .read_source(OplogIndex::INITIAL, 2)
            .await
            .map(|read| read.into_keys().collect::<Vec<_>>()),
        Ok(vec![OplogIndex::INITIAL, OplogIndex::from_u64(2)]),
        "the new owner must read the chunk the manifest lists; the append returned {appended:?}"
    );
    assert_eq!(
        relay.appends.load(Ordering::SeqCst),
        1,
        "the manifest append was sent again, and returned {appended:?}"
    );
    assert!(
        appended.is_ok(),
        "the entry was stored, so the append must reconcile as stored: {appended:?}"
    );
    relay.server.abort();
}
