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
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{LogLevel, OplogEntry, OplogIndex};
use golem_common::model::{AgentId, OwnedAgentId, ScanCursor};
use golem_service_base::config::{S3BlobStorageConfig, S3BlobStorageCredentialsConfig};
use golem_service_base::storage::blob::{BlobStorage, s3};
use golem_worker_executor::services::oplog::{BlobOplogArchiveService, OplogArchiveService};
use pretty_assertions::assert_eq;
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;
use test_r::{define_matrix_dimension, test, test_dep};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::Mutex;

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

/// Spins up a fresh MinIO container per `get_blob_storage` call and keeps it
/// alive for the lifetime of this (per-worker) dependency, so the returned S3
/// blob storage remains usable for the whole test.
struct S3Test {
    containers: Mutex<Vec<ContainerAsync<GenericImage>>>,
}

impl Debug for S3Test {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "S3Test")
    }
}

#[async_trait]
impl GetBlobStorage for S3Test {
    async fn get_blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync> {
        let container = tryhard::retry_fn(|| {
            GenericImage::new("minio/minio", "RELEASE.2025-01-20T14-49-07Z")
                .with_exposed_port(9000.tcp())
                .with_wait_for(WaitFor::message_on_stderr("API:"))
                .with_env_var("MINIO_CONSOLE_ADDRESS", ":9001")
                .with_cmd(["server", "/data"])
                .start()
        })
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start MinIO");
        let host_port = container
            .get_host_port_ipv4(9000)
            .await
            .expect("Failed to get host port");

        let config = S3BlobStorageConfig {
            retries: Default::default(),
            region: "us-east-1".to_string(),
            object_prefix: String::new(),
            aws_endpoint_url: Some(format!("http://127.0.0.1:{host_port}")),
            aws_credentials: Some(S3BlobStorageCredentialsConfig::new(
                "minioadmin",
                "minioadmin",
                "test",
            )),
            ..std::default::Default::default()
        };
        create_buckets(host_port, &config).await;
        let storage = s3::S3BlobStorage::new(config).await;

        self.containers.lock().await.push(container);
        Arc::new(storage)
    }
}

async fn create_buckets(host_port: u16, config: &S3BlobStorageConfig) {
    let endpoint_uri = format!("http://127.0.0.1:{host_port}");
    let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");
    let creds = Credentials::new("minioadmin", "minioadmin", None, None, "test");
    let sdk_config = aws_config::defaults(BehaviorVersion::latest())
        .region(region_provider)
        .endpoint_url(endpoint_uri)
        .credentials_provider(creds)
        .load()
        .await;

    let client = Client::new(&sdk_config);
    for bucket in &config.compressed_oplog_buckets {
        client.create_bucket().bucket(bucket).send().await.unwrap();
    }
}

#[test_dep(scope = PerWorker, tagged_as = "s3")]
fn s3() -> Arc<dyn GetBlobStorage + Send + Sync> {
    Arc::new(S3Test {
        containers: Mutex::new(Vec::new()),
    })
}

define_matrix_dimension!(storage: Arc<dyn GetBlobStorage + Send + Sync> -> "in_memory", "s3");

async fn append_worker(
    service: &BlobOplogArchiveService,
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
) {
    let archive = service.open(owned_agent_id, agent_mode).await;
    archive
        .append(vec![(
            OplogIndex::INITIAL,
            OplogEntry::log(LogLevel::Debug, "test".to_string(), "test".to_string()),
        )])
        .await;
}

async fn drain(
    service: &BlobOplogArchiveService,
    environment_id: &EnvironmentId,
    component_id: &ComponentId,
    modes: Option<AgentMode>,
) -> Vec<OwnedAgentId> {
    let mut cursor = ScanCursor::default();
    let mut acc: Vec<OwnedAgentId> = Vec::new();
    loop {
        let (next_cursor, ids) = service
            .scan_for_component(environment_id, component_id, modes, cursor, 100)
            .await
            .unwrap();
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
/// S3-compatible (MinIO) backend.
#[test]
async fn blob_archive_scan_for_component_filters_by_mode(
    #[dimension(storage)] storage: &Arc<dyn GetBlobStorage + Send + Sync>,
) {
    let blob_storage = storage.get_blob_storage().await;

    // `compressed_oplog_buckets[0]` ("oplog-archive-1") corresponds to level 0.
    let service = BlobOplogArchiveService::new(blob_storage, 0);

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
            Some(AgentMode::Ephemeral)
        )
        .await,
        expected_ephemeral
    );
    assert_eq!(
        drain(
            &service,
            &environment_id,
            &component_id,
            Some(AgentMode::Durable)
        )
        .await,
        expected_durable
    );
    assert_eq!(
        drain(&service, &environment_id, &component_id, None).await,
        expected_both
    );

    // A component with no archived workers yields an empty scan.
    let empty_component_id = ComponentId::new();
    assert_eq!(
        drain(&service, &environment_id, &empty_component_id, None).await,
        Vec::<OwnedAgentId>::new()
    );
}
