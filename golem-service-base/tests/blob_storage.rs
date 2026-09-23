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

use anyhow::Error;
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_config::meta::region::RegionProviderChain;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::Credentials;
use bytes::{BufMut, Bytes, BytesMut};
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use golem_common::model::AgentId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::widen_infallible;
use golem_service_base::config::{S3BlobStorageConfig, S3BlobStorageCredentialsConfig};
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::*;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace, fs, memory, s3};
use pretty_assertions::assert_eq;
use sqlx::sqlite::SqlitePoolOptions;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;
use tempfile::{TempDir, tempdir};
use test_r::{define_matrix_dimension, test, test_dep};
use testcontainers::ContainerAsync;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt};
use uuid::Uuid;

#[async_trait]
trait GetBlobStorage: Debug {
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
        Arc::new(memory::InMemoryBlobStorage::new())
    }
}

#[test_dep(scope = PerWorker, tagged_as = "in_memory")]
fn in_memory() -> Arc<dyn GetBlobStorage + Send + Sync> {
    Arc::new(InMemoryTest)
}

struct FsTest {
    dir: TempDir,
    counter: AtomicU32,
}

impl Debug for FsTest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FsTest")
    }
}

#[async_trait]
impl GetBlobStorage for FsTest {
    async fn get_blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync> {
        let counter = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = self.dir.path().join(format!("test-{counter}"));
        Arc::new(fs::FileSystemBlobStorage::new(&path).await.unwrap())
    }
}

#[test_dep(scope = PerWorker, tagged_as = "fs")]
async fn fs() -> Arc<dyn GetBlobStorage + Send + Sync> {
    let dir = tempdir().unwrap();
    let counter = AtomicU32::new(0);
    Arc::new(FsTest { dir, counter })
}

struct S3Test {
    prefixed: Option<String>,
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
            object_prefix: self.prefixed.clone().unwrap_or_default(),
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
        Arc::new(S3BlobStorageWithContainer {
            storage,
            _container: container,
        })
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
    client
        .create_bucket()
        .bucket(&config.compilation_cache_bucket)
        .send()
        .await
        .unwrap();
    client
        .create_bucket()
        .bucket(&config.custom_data_bucket)
        .send()
        .await
        .unwrap();
    client
        .create_bucket()
        .bucket(&config.oplog_payload_bucket)
        .send()
        .await
        .unwrap();
    client
        .create_bucket()
        .bucket(&config.filesystem_snapshots_bucket)
        .send()
        .await
        .unwrap();
    for bucket in &config.compressed_oplog_buckets {
        client.create_bucket().bucket(bucket).send().await.unwrap();
    }
}

struct S3BlobStorageWithContainer {
    storage: s3::S3BlobStorage,
    _container: ContainerAsync<GenericImage>,
}

impl Debug for S3BlobStorageWithContainer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "S3BlobStorageWithContainer")
    }
}

#[async_trait]
impl BlobStorage for S3BlobStorageWithContainer {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.storage
            .get_raw(target_label, op_label, namespace, path)
            .await
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, Error>>>, Error> {
        self.storage
            .get_stream(target_label, op_label, namespace, path)
            .await
    }

    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.storage
            .get_raw_slice(target_label, op_label, namespace, path, start, end)
            .await
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Option<BlobMetadata>, Error> {
        self.storage
            .get_metadata(target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<(), Error> {
        self.storage
            .put_raw(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_raw_if_absent(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        self.storage
            .put_raw_if_absent(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        self.storage
            .put_stream(target_label, op_label, namespace, path, stream)
            .await
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        self.storage
            .delete(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_many(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> Result<(), Error> {
        self.storage
            .delete_many(target_label, op_label, namespace, paths)
            .await
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<(), Error> {
        self.storage
            .create_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Vec<PathBuf>, Error> {
        self.storage
            .list_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_blobs_below(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<Box<[ListedBlob]>, Error> {
        self.storage
            .list_blobs_below(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, Error> {
        self.storage
            .delete_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<ExistsResult, Error> {
        self.storage
            .exists(target_label, op_label, namespace, path)
            .await
    }

    async fn copy(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
        self.storage
            .copy(target_label, op_label, namespace, from, to)
            .await
    }

    async fn r#move(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> Result<(), Error> {
        self.storage
            .r#move(target_label, op_label, namespace, from, to)
            .await
    }
}

#[test_dep(scope = PerWorker, tagged_as = "s3")]
async fn s3() -> Arc<dyn GetBlobStorage + Send + Sync> {
    Arc::new(S3Test { prefixed: None })
}

#[test_dep(scope = PerWorker, tagged_as = "s3_prefixed")]
async fn s3_prefixed() -> Arc<dyn GetBlobStorage + Send + Sync> {
    Arc::new(S3Test {
        prefixed: Some("random-prefix".to_string()),
    })
}

struct SqliteTest;

impl Debug for SqliteTest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SqliteTest")
    }
}

#[async_trait]
impl GetBlobStorage for SqliteTest {
    async fn get_blob_storage(&self) -> Arc<dyn BlobStorage + Send + Sync> {
        let sqlx_pool_sqlite = SqlitePoolOptions::new()
            .min_connections(10)
            .max_connections(10)
            .connect("sqlite::memory:")
            .await
            .expect("Cannot create db options");

        let pool = SqlitePool::new(sqlx_pool_sqlite.clone(), sqlx_pool_sqlite.clone());
        let sbs = SqliteBlobStorage::new(pool).await.unwrap();
        Arc::new(sbs)
    }
}

#[test_dep(scope = PerWorker, tagged_as = "sqlite")]
async fn sqlite() -> Arc<dyn GetBlobStorage + Send + Sync> {
    Arc::new(SqliteTest)
}

#[test_dep(scope = PerWorker, tagged_as = "cc")]
fn compilation_cache() -> BlobStorageNamespace {
    BlobStorageNamespace::CompilationCache {
        environment_id: EnvironmentId(
            Uuid::parse_str("4c8c5ff4-2a42-4e81-ac48-e63005f609fd").unwrap(),
        ),
    }
}

#[test_dep(scope = PerWorker, tagged_as = "co")]
fn compressed_oplog() -> BlobStorageNamespace {
    BlobStorageNamespace::CompressedOplog {
        environment_id: EnvironmentId(
            Uuid::parse_str("4c8c5ff4-2a42-4e81-ac48-e63005f609fd").unwrap(),
        ),
        component_id: ComponentId(Uuid::new_v4()),
        agent_mode: AgentMode::Durable,
        level: 0,
    }
}

#[test_dep(scope = PerWorker, tagged_as = "cs")]
fn custom_storage() -> BlobStorageNamespace {
    BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId(
            Uuid::parse_str("4c8c5ff4-2a42-4e81-ac48-e63005f609fd").unwrap(),
        ),
    }
}

/// The filesystem snapshots of one agent whose name holds a `..` segment. The rules of a blob
/// name refuse such a segment, so the location of the agent must not hold its name.
#[test_dep(scope = PerWorker, tagged_as = "fss")]
fn filesystem_snapshots() -> BlobStorageNamespace {
    filesystem_snapshots_of(
        "4c8c5ff4-2a42-4e81-ac48-e63005f609fd",
        "7e0e4c9a-3c34-4d52-8d6f-0d2f6b6d3a11",
        r#"counter("a/../b")"#,
    )
}

/// The filesystem snapshots namespace of the agent `agent` of the component `component` in the
/// environment `environment`.
fn filesystem_snapshots_of(
    environment: &str,
    component: &str,
    agent: &str,
) -> BlobStorageNamespace {
    BlobStorageNamespace::FilesystemSnapshots {
        environment_id: EnvironmentId(Uuid::parse_str(environment).unwrap()),
        agent_id: AgentId {
            component_id: ComponentId(Uuid::parse_str(component).unwrap()),
            agent_id: agent.to_string(),
        },
    }
}

define_matrix_dimension!(storage: Arc<dyn GetBlobStorage + Send + Sync> -> "in_memory", "fs", "s3", "s3_prefixed", "sqlite");
// The in-memory backend stands in for S3 in the tests of other crates, so the two must give the
// same answer. The filesystem and the SQLite backends do not give it for every operation yet.
define_matrix_dimension!(mem_and_s3: Arc<dyn GetBlobStorage + Send + Sync> -> "in_memory", "s3", "s3_prefixed");
define_matrix_dimension!(ns: BlobStorageNamespace -> "cc", "co", "cs");
define_matrix_dimension!(s3_storage: Arc<dyn GetBlobStorage + Send + Sync> -> "s3", "s3_prefixed");

/// The paths that are at the root of a namespace, because none of them has a name in it.
const ROOT_PATHS: [&str; 4] = ["", ".", "./", "././"];

#[test]
#[tracing::instrument]
async fn get_put_get_root(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let path = Path::new("test-path");
    let data = Bytes::from("test-data").to_vec();

    let result1 = storage
        .get_raw("get_put_get_root", "get-raw", namespace.clone(), path)
        .await
        .unwrap();

    storage
        .put_raw(
            "get_put_get_root",
            "put-raw",
            namespace.clone(),
            path,
            &data,
        )
        .await
        .unwrap();

    let result2 = storage
        .get_raw("get_put_get_root", "get-raw-2", namespace.clone(), path)
        .await
        .unwrap();

    assert_eq!(result1, None);
    assert_eq!(result2, Some(data));
}

#[test]
#[tracing::instrument]
async fn get_put_get_new_dir(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let path = Path::new("non-existing-dir/test-path");
    let data = Bytes::from("test-data").to_vec();

    let result1 = storage
        .get_raw("get_put_get_new_dir", "get-raw", namespace.clone(), path)
        .await
        .unwrap();

    storage
        .put_raw(
            "get_put_get_new_dir",
            "put-raw",
            namespace.clone(),
            path,
            &data,
        )
        .await
        .unwrap();

    let result2 = storage
        .get_raw("get_put_get_new_dir", "get-raw-2", namespace.clone(), path)
        .await
        .unwrap();

    assert_eq!(result1, None);
    assert_eq!(result2, Some(data));
}

#[test]
#[tracing::instrument]
async fn get_put_get_new_dir_streaming(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let path = Path::new("non-existing-dir/test-path");

    let result1 = storage
        .get_stream("get_put_get_new_dir", "get-raw", namespace.clone(), path)
        .await
        .unwrap();

    let mut data = BytesMut::new();
    for n in 1..(10 * 1024 * 1024) {
        data.put_u8((n % 100) as u8);
    }
    let data = data.freeze().to_vec();

    let stream = (&data)
        .map_item(|i| i.map_err(widen_infallible))
        .map_error(widen_infallible)
        .erased();

    storage
        .put_stream(
            "get_put_get_new_dir",
            "put-raw",
            namespace.clone(),
            path,
            &stream,
        )
        .await
        .unwrap();

    let result2 = storage
        .get_stream("get_put_get_new_dir", "get-raw-2", namespace.clone(), path)
        .await
        .unwrap()
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .concat();

    assert!(result1.is_none());
    assert_eq!(result2, data.to_vec());
}

#[test]
#[tracing::instrument]
async fn create_delete_exists_dir(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let path = Path::new("test-dir");

    let result1 = storage
        .exists(
            "create_delete_exists_dir",
            "exists",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();
    storage
        .create_dir(
            "create_delete_exists_dir",
            "create-dir",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();
    let result2 = storage
        .exists(
            "create_delete_exists_dir",
            "exists-2",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();
    let delete_result1 = storage
        .delete_dir(
            "create_delete_exists_dir",
            "delete-dir",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();
    let result3 = storage
        .exists(
            "create_delete_exists_dir",
            "exists-3",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();
    let delete_result2 = storage
        .delete_dir(
            "create_delete_exists_dir",
            "delete-dir",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();

    assert_eq!(result1, ExistsResult::DoesNotExist);
    assert_eq!(result2, ExistsResult::Directory);
    assert_eq!(result3, ExistsResult::DoesNotExist);
    assert_eq!(delete_result1, true);
    assert_eq!(delete_result2, false);
}

#[test]
#[tracing::instrument]
async fn create_delete_exists_dir_and_file(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let path = Path::new("test-dir");

    let result1 = storage
        .exists(
            "create_delete_exists_dir_and_file",
            "exists",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();
    storage
        .create_dir(
            "create_delete_exists_dir_and_file",
            "create-dir",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "create_delete_exists_dir_and_file",
            "put-raw",
            namespace.clone(),
            &path.join("test-file"),
            &Bytes::from("test-data"),
        )
        .await
        .unwrap();
    let result2 = storage
        .exists(
            "create_delete_exists_dir_and_file",
            "exists-2",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();
    let result3 = storage
        .exists(
            "create_delete_exists_dir_and_file",
            "exists-3",
            namespace.clone(),
            &path.join("test-file"),
        )
        .await
        .unwrap();
    storage
        .delete_dir(
            "create_delete_exists_dir_and_file",
            "delete-dir",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();
    let result4 = storage
        .exists(
            "create_delete_exists_dir_and_file",
            "exists-4",
            namespace.clone(),
            path,
        )
        .await
        .unwrap();

    assert_eq!(result1, ExistsResult::DoesNotExist);
    assert_eq!(result2, ExistsResult::Directory);
    assert_eq!(result3, ExistsResult::File);
    assert_eq!(result4, ExistsResult::DoesNotExist);
}

#[test]
#[tracing::instrument]
async fn in_memory_prefix_sibling_does_not_hide_descendants(
    #[tagged_as("in_memory")] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    storage
        .put_raw(
            "in_memory_prefix_sibling_does_not_hide_descendants",
            "put-sibling",
            namespace.clone(),
            Path::new("a-/blob"),
            b"data",
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "in_memory_prefix_sibling_does_not_hide_descendants",
            "put-descendant",
            namespace.clone(),
            Path::new("a/b/blob"),
            b"data",
        )
        .await
        .unwrap();

    assert_eq!(
        storage
            .exists(
                "in_memory_prefix_sibling_does_not_hide_descendants",
                "exists",
                namespace.clone(),
                Path::new("a"),
            )
            .await
            .unwrap(),
        ExistsResult::Directory
    );
    storage
        .create_dir(
            "in_memory_prefix_sibling_does_not_hide_descendants",
            "create-dir",
            namespace.clone(),
            Path::new("a"),
        )
        .await
        .unwrap();
    assert!(
        storage
            .delete_dir(
                "in_memory_prefix_sibling_does_not_hide_descendants",
                "delete-dir",
                namespace.clone(),
                Path::new("a"),
            )
            .await
            .unwrap()
    );
    assert_eq!(
        storage
            .get_raw(
                "in_memory_prefix_sibling_does_not_hide_descendants",
                "get-deleted",
                namespace.clone(),
                Path::new("a/b/blob"),
            )
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        storage
            .get_raw(
                "in_memory_prefix_sibling_does_not_hide_descendants",
                "get-sibling",
                namespace.clone(),
                Path::new("a-/blob"),
            )
            .await
            .unwrap(),
        Some(b"data".to_vec())
    );
}

#[test]
#[tracing::instrument]
async fn list_dir(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let path = Path::new("test-dir");
    storage
        .create_dir("list_dir", "create-dir", namespace.clone(), path)
        .await
        .unwrap();
    storage
        .put_raw(
            "list_dir",
            "put-raw",
            namespace.clone(),
            &path.join("test-file1"),
            &Bytes::from("test-data1"),
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "list_dir",
            "put-raw",
            namespace.clone(),
            &path.join("test-file2"),
            &Bytes::from("test-data2"),
        )
        .await
        .unwrap();
    storage
        .create_dir(
            "list_dir",
            "create-dir",
            namespace.clone(),
            &path.join("inner-dir"),
        )
        .await
        .unwrap();
    let mut entries = storage
        .list_dir("list_dir", "entries", namespace.clone(), path)
        .await
        .unwrap();

    entries.sort();

    assert_eq!(
        entries,
        vec![
            Path::new("test-dir/inner-dir").to_path_buf(),
            Path::new("test-dir/test-file1").to_path_buf(),
            Path::new("test-dir/test-file2").to_path_buf(),
        ]
    );
}

#[test]
#[tracing::instrument]
async fn delete_many(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let path = Path::new("test-dir");
    storage
        .create_dir("list_dir", "create-dir", namespace.clone(), path)
        .await
        .unwrap();
    storage
        .put_raw(
            "delete_many",
            "put-raw",
            namespace.clone(),
            &path.join("test-file1"),
            &Bytes::from("test-data1"),
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "delete_many",
            "put-raw",
            namespace.clone(),
            &path.join("test-file2"),
            &Bytes::from("test-data2"),
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "delete_many",
            "put-raw",
            namespace.clone(),
            &path.join("test-file3"),
            &Bytes::from("test-data3"),
        )
        .await
        .unwrap();
    storage
        .create_dir(
            "delete_many",
            "create-dir",
            namespace.clone(),
            &path.join("inner-dir"),
        )
        .await
        .unwrap();
    storage
        .delete_many(
            "delete_many",
            "delete-many",
            namespace.clone(),
            &[path.join("test-file1"), path.join("test-file3")],
        )
        .await
        .unwrap();

    let mut entries = storage
        .list_dir("delete_many", "entries", namespace.clone(), path)
        .await
        .unwrap();

    entries.sort();

    assert_eq!(
        entries,
        vec![
            Path::new("test-dir/inner-dir").to_path_buf(),
            Path::new("test-dir/test-file2").to_path_buf(),
        ]
    );
}

#[test]
#[tracing::instrument]
async fn delete_many_reads_every_path_before_it_removes_a_blob(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    // The S3 backend makes the key of every path before it sends a request, so a path that
    // breaks a rule of a name removes no blob there. A backend that removes the blob of one
    // path at a time has to give the same answer, because the in-memory backend stands in for
    // S3 in the tests of other crates. The good path comes first, so a backend that reads each
    // path only when it removes its blob removes that blob and then gives the error.
    let storage = test.get_blob_storage().await;
    let label = "delete_many_reads_every_path_before_it_removes_a_blob";
    let good = Path::new("good");
    let bad = Path::new("../bad");

    storage
        .put_raw(label, "put-raw", namespace.clone(), good, b"payload")
        .await
        .unwrap();

    let rule = storage
        .delete_many(
            label,
            "delete-many",
            namespace.clone(),
            &[good.to_path_buf(), bad.to_path_buf()],
        )
        .await
        .err()
        .and_then(name_error);

    assert_eq!(
        (
            rule,
            storage
                .get_raw(label, "get-raw", namespace.clone(), good)
                .await
                .unwrap()
        ),
        (
            Some(BlobNameError::ParentDir {
                path: bad.to_path_buf()
            }),
            Some(b"payload".to_vec())
        )
    );
}

#[test]
#[tracing::instrument]
async fn list_dir_root(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    storage
        .put_raw(
            "list_dir_root",
            "put-raw",
            namespace.clone(),
            Path::new("test-file1"),
            &Bytes::from("test-data1"),
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "list_dir_root",
            "put-raw-2",
            namespace.clone(),
            Path::new("test-file2"),
            &Bytes::from("test-data2"),
        )
        .await
        .unwrap();
    storage
        .create_dir(
            "list_dir_root",
            "create-dir",
            namespace.clone(),
            Path::new("inner-dir"),
        )
        .await
        .unwrap();
    let mut entries = storage
        .list_dir(
            "list_dir_root",
            "list-dir",
            namespace.clone(),
            Path::new(""),
        )
        .await
        .unwrap();

    entries.sort();

    assert_eq!(
        entries,
        vec![
            Path::new("inner-dir").to_path_buf(),
            Path::new("test-file1").to_path_buf(),
            Path::new("test-file2").to_path_buf(),
        ]
    );
}

#[test]
#[tracing::instrument]
async fn list_dir_root_only_subdirs(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    storage
        .create_dir(
            "list_dir_root",
            "create-dir",
            namespace.clone(),
            Path::new("inner-dir1"),
        )
        .await
        .unwrap();
    storage
        .create_dir(
            "list_dir_root",
            "create-dir",
            namespace.clone(),
            Path::new("inner-dir2"),
        )
        .await
        .unwrap();

    storage
        .put_raw(
            "list_dir_root",
            "put-raw",
            namespace.clone(),
            Path::new("inner-dir1/test-file1"),
            &Bytes::from("test-data1"),
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "list_dir_root",
            "put-raw-2",
            namespace.clone(),
            Path::new("inner-dir2/test-file2"),
            &Bytes::from("test-data2"),
        )
        .await
        .unwrap();
    let mut entries = storage
        .list_dir(
            "list_dir_root",
            "list-dir",
            namespace.clone(),
            Path::new(""),
        )
        .await
        .unwrap();

    entries.sort();

    assert_eq!(
        entries,
        vec![
            Path::new("inner-dir1").to_path_buf(),
            Path::new("inner-dir2").to_path_buf(),
        ]
    );
}

#[test]
#[tracing::instrument]
async fn list_dir_gives_a_created_directory_below_the_path_at_any_depth(
    #[dimension(mem_and_s3)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    // The in-memory backend holds the key of a directory that `create_dir` made, and the S3
    // backend holds the marker object of it. Each of them reads every such key below the path,
    // so a directory two names below the path is in the list with both names. A directory that
    // only holds blobs has no key and no marker, so `dir/blobs` is not in the list, and neither
    // is the blob that is below it.
    let storage = test.get_blob_storage().await;

    storage
        .create_dir(
            "list_dir_gives_a_created_directory_below_the_path_at_any_depth",
            "create-dir",
            namespace.clone(),
            Path::new("dir/sub/deep"),
        )
        .await
        .unwrap();
    put_blobs(
        &storage,
        namespace,
        &[("dir/blob", 1), ("dir/blobs/nested", 1)],
    )
    .await;

    let mut entries = storage
        .list_dir(
            "list_dir_gives_a_created_directory_below_the_path_at_any_depth",
            "list-dir",
            namespace.clone(),
            Path::new("dir"),
        )
        .await
        .unwrap();
    entries.sort();

    assert_eq!(
        entries,
        vec![PathBuf::from("dir/blob"), PathBuf::from("dir/sub/deep"),]
    );
}

#[test]
#[tracing::instrument]
async fn list_dir_gives_a_blob_and_the_directory_of_its_path_one_time(
    #[dimension(mem_and_s3)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    // A blob and a directory that `create_dir` made can hold one path. The in-memory backend
    // then holds two keys for the path, and the S3 backend holds the blob and the marker of the
    // directory. `list_dir` gives the path one time.
    let storage = test.get_blob_storage().await;
    let label = "list_dir_gives_a_blob_and_the_directory_of_its_path_one_time";

    storage
        .create_dir(label, "create-dir", namespace.clone(), Path::new("a"))
        .await
        .unwrap();
    put_blobs(&storage, namespace, &[("a", 5)]).await;

    assert_eq!(
        storage
            .list_dir(label, "list-root", namespace.clone(), Path::new(""))
            .await
            .unwrap(),
        vec![PathBuf::from("a")]
    );

    // A delete of the blob removes no marker and no key of a directory, so the directory is
    // still there and the path stays in the list.
    storage
        .delete(label, "delete-blob", namespace.clone(), Path::new("a"))
        .await
        .unwrap();

    assert_eq!(
        storage
            .list_dir(label, "list-root", namespace.clone(), Path::new(""))
            .await
            .unwrap(),
        vec![PathBuf::from("a")]
    );
}

#[test]
#[tracing::instrument]
async fn list_dir_same_prefix(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let path1 = Path::new("test-dir");
    let path2 = Path::new("test-dir2");
    let path3 = Path::new("test-dir3");
    storage
        .create_dir("list_dir", "create-dir", namespace.clone(), path1)
        .await
        .unwrap();
    storage
        .create_dir("list_dir", "create-dir-2", namespace.clone(), path2)
        .await
        .unwrap();
    storage
        .create_dir("list_dir", "create-dir-3", namespace.clone(), path3)
        .await
        .unwrap();
    storage
        .put_raw(
            "list_dir_same_prefix",
            "put-raw",
            namespace.clone(),
            &path1.join("test-file1"),
            &Bytes::from("test-data1"),
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "list_dir_same_prefix",
            "put-raw",
            namespace.clone(),
            &path1.join("test-file2"),
            &Bytes::from("test-data2"),
        )
        .await
        .unwrap();
    storage
        .create_dir(
            "list_dir_same_prefix",
            "create-dir",
            namespace.clone(),
            &path1.join("inner-dir"),
        )
        .await
        .unwrap();
    let mut entries = storage
        .list_dir("list_dir_same_prefix", "entries", namespace.clone(), path1)
        .await
        .unwrap();

    entries.sort();

    assert_eq!(
        entries,
        vec![
            Path::new("test-dir/inner-dir").to_path_buf(),
            Path::new("test-dir/test-file1").to_path_buf(),
            Path::new("test-dir/test-file2").to_path_buf(),
        ]
    );
}

#[test]
#[tracing::instrument]
async fn delete_dir_must_not_delete_siblings(
    #[tagged_as("sqlite")] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let dir_a = Path::new("dir-a");
    let dir_b = Path::new("dir-b");

    storage
        .put_raw(
            "delete_dir_must_not_delete_siblings",
            "put-a",
            namespace.clone(),
            &dir_a.join("file-a"),
            &Bytes::from("data-a"),
        )
        .await
        .unwrap();

    storage
        .put_raw(
            "delete_dir_must_not_delete_siblings",
            "put-b",
            namespace.clone(),
            &dir_b.join("file-b"),
            &Bytes::from("data-b"),
        )
        .await
        .unwrap();

    storage
        .delete_dir(
            "delete_dir_must_not_delete_siblings",
            "delete-a",
            namespace.clone(),
            dir_a,
        )
        .await
        .unwrap();

    let remaining = storage
        .get_raw(
            "delete_dir_must_not_delete_siblings",
            "get-b",
            namespace.clone(),
            &dir_b.join("file-b"),
        )
        .await
        .unwrap();

    assert_eq!(remaining, Some(Bytes::from("data-b").to_vec()));
}

#[test]
#[tracing::instrument]
async fn fs_rejects_parent_traversal(
    #[tagged_as("fs")] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let result = storage
        .put_raw(
            "fs_rejects_parent_traversal",
            "put-raw",
            namespace.clone(),
            Path::new("../../../../escape"),
            &Bytes::from("payload"),
        )
        .await;

    assert!(result.is_err());
}

#[test]
#[tracing::instrument]
async fn fs_list_blobs_below_fails_for_a_directory_that_it_cannot_read(
    #[tagged_as("fs")] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[tagged_as("cs")] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    // Writing a blob creates the directory of the namespace. Below it, the test lists a name of
    // 300 bytes. The filesystem that holds the namespace must not accept a name of that length,
    // so the directory read gives an error that is not "not found" and not "not a directory".
    put_blobs(&storage, namespace, &[("blob", 1)]).await;
    let too_long = "x".repeat(300);

    let result = storage
        .list_blobs_below(
            "fs_list_blobs_below_fails_for_a_directory_that_it_cannot_read",
            "list",
            namespace.clone(),
            Path::new(&too_long),
        )
        .await;

    assert!(result.is_err(), "{result:?}");
}

#[test]
#[tracing::instrument]
async fn reject_parent_traversal_in_put_raw(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let result = storage
        .put_raw(
            "reject_parent_traversal_in_put_raw",
            "put-raw",
            namespace.clone(),
            Path::new("../escape"),
            &Bytes::from("payload"),
        )
        .await;

    assert!(result.is_err());
}

/// Gives the `BlobNameError` of an error of the blob storage, or `None` for another error.
///
/// `blob_store_error` in `golem_worker_executor::services::blob_store` downcasts the same way
/// and makes a `BlobStoreError::InvalidInput` of what it gets. `classify_blob_store_error` in
/// `golem_worker_executor::durable_host::blobstore` makes that permanent, so the guest gets the
/// error at once and the executor does not retry it.
fn name_error(error: Error) -> Option<BlobNameError> {
    error.downcast_ref::<BlobNameError>().cloned()
}

#[test]
#[tracing::instrument]
async fn a_path_that_starts_with_a_prefix_of_windows_is_not_relative(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    // `C:` names a drive on Windows and `\\` names a server, so a path that starts with one of
    // them names a place outside the namespace on that host. The rule reads the text of the
    // path and not the host, so each backend refuses the path on each operating system.
    let storage = test.get_blob_storage().await;
    let label = "a_path_that_starts_with_a_prefix_of_windows_is_not_relative";

    for name in ["C:/escape", "C:\\escape", "C:escape", "\\\\server\\share"] {
        let path = Path::new(name);
        let rule = BlobNameError::NotRelative {
            path: path.to_path_buf(),
        };

        let written = storage
            .put_raw(label, "put-raw", namespace.clone(), path, b"payload")
            .await
            .err()
            .and_then(name_error);
        let created = storage
            .create_dir(label, "create-dir", namespace.clone(), path)
            .await
            .err()
            .and_then(name_error);
        let read = storage
            .get_raw(label, "get-raw", namespace.clone(), path)
            .await
            .err()
            .and_then(name_error);

        assert_eq!(
            (written, created, read),
            (Some(rule.clone()), Some(rule.clone()), Some(rule)),
            "the name {name:?}"
        );
    }

    assert_eq!(
        storage
            .list_dir(label, "list-root", namespace.clone(), Path::new(""))
            .await
            .unwrap(),
        Vec::<PathBuf>::new(),
        "a name that breaks a rule left an entry behind"
    );
}

#[test]
#[tracing::instrument]
async fn a_name_that_breaks_a_rule_of_the_storage_gives_that_rule(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    // Each of these rules is a rule of S3 or of MinIO, and `normalized_blob_path` applies it,
    // so each backend gives the same error for the same name. The in-memory backend stands in
    // for S3 in the tests of other crates, so a name that passes there must be a name that S3
    // accepts.
    let storage = test.get_blob_storage().await;
    let label = "a_name_that_breaks_a_rule_of_the_storage_gives_that_rule";
    let names = [
        ("a\0b", BlobNameError::NulByte),
        (
            " .. ",
            BlobNameError::DotSegment {
                segment: " .. ".to_string(),
            },
        ),
        (
            "a\\..\\b",
            BlobNameError::DotSegment {
                segment: "..".to_string(),
            },
        ),
        (
            "dir/__dir_marker",
            BlobNameError::Reserved {
                marker: "__dir_marker",
            },
        ),
    ];

    for (name, rule) in names {
        let path = Path::new(name);

        let written = storage
            .put_raw(label, "put-raw", namespace.clone(), path, b"payload")
            .await
            .err()
            .and_then(name_error);
        let written_if_absent = storage
            .put_raw_if_absent(label, "put-if-absent", namespace.clone(), path, b"payload")
            .await
            .err()
            .and_then(name_error);
        let created = storage
            .create_dir(label, "create-dir", namespace.clone(), path)
            .await
            .err()
            .and_then(name_error);
        let read = storage
            .get_raw(label, "get-raw", namespace.clone(), path)
            .await
            .err()
            .and_then(name_error);

        assert_eq!(
            (written, written_if_absent, created, read),
            (
                Some(rule.clone()),
                Some(rule.clone()),
                Some(rule.clone()),
                Some(rule)
            ),
            "the name {name:?}"
        );
    }

    assert_eq!(
        storage
            .list_dir(label, "list-root", namespace.clone(), Path::new(""))
            .await
            .unwrap(),
        Vec::<PathBuf>::new(),
        "a name that breaks a rule left an entry behind"
    );
}

#[test]
#[tracing::instrument]
async fn delete_dir_escapes_like_wildcards(
    #[tagged_as("sqlite")] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let wildcard_dir = Path::new("dir%name");
    let sibling_dir = Path::new("dirXname");

    storage
        .put_raw(
            "delete_dir_escapes_like_wildcards",
            "put-a",
            namespace.clone(),
            &wildcard_dir.join("file-a"),
            &Bytes::from("data-a"),
        )
        .await
        .unwrap();

    storage
        .put_raw(
            "delete_dir_escapes_like_wildcards",
            "put-b",
            namespace.clone(),
            &sibling_dir.join("file-b"),
            &Bytes::from("data-b"),
        )
        .await
        .unwrap();

    storage
        .delete_dir(
            "delete_dir_escapes_like_wildcards",
            "delete-a",
            namespace.clone(),
            wildcard_dir,
        )
        .await
        .unwrap();

    let remaining = storage
        .get_raw(
            "delete_dir_escapes_like_wildcards",
            "get-b",
            namespace.clone(),
            &sibling_dir.join("file-b"),
        )
        .await
        .unwrap();

    assert_eq!(remaining, Some(Bytes::from("data-b").to_vec()));
}

#[test]
#[tracing::instrument]
async fn s3_copy_handles_reserved_characters_in_source_key(
    #[tagged_as("s3")] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let from = Path::new("dir with spaces/source #file?.txt");
    let to = Path::new("target/renamed.txt");
    let payload = Bytes::from("copy-payload").to_vec();

    storage
        .put_raw(
            "s3_copy_handles_reserved_characters_in_source_key",
            "put-raw",
            namespace.clone(),
            from,
            &payload,
        )
        .await
        .unwrap();

    storage
        .copy(
            "s3_copy_handles_reserved_characters_in_source_key",
            "copy",
            namespace.clone(),
            from,
            to,
        )
        .await
        .unwrap();

    let copied = storage
        .get_raw(
            "s3_copy_handles_reserved_characters_in_source_key",
            "get-raw",
            namespace.clone(),
            to,
        )
        .await
        .unwrap();

    assert_eq!(copied, Some(payload));
}

#[test]
#[tracing::instrument]
async fn delete_dir_does_not_delete_file_with_same_path(
    #[tagged_as("sqlite")] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let file_path = Path::new("not-a-dir");

    storage
        .put_raw(
            "delete_dir_does_not_delete_file_with_same_path",
            "put-file",
            namespace.clone(),
            file_path,
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    let deleted = storage
        .delete_dir(
            "delete_dir_does_not_delete_file_with_same_path",
            "delete-dir",
            namespace.clone(),
            file_path,
        )
        .await
        .unwrap();

    assert!(!deleted);

    let remaining = storage
        .get_raw(
            "delete_dir_does_not_delete_file_with_same_path",
            "get-file",
            namespace.clone(),
            file_path,
        )
        .await
        .unwrap();

    assert_eq!(remaining, Some(Bytes::from("payload").to_vec()));
}

// Regression test for https://github.com/golemcloud/golem/issues/3280
// clear() (delete_dir + create_dir) must leave the container directory intact so that a
// subsequent list_dir does not fail with "No such file or directory".
#[test]
#[tracing::instrument]
async fn clear_then_list_objects(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let container = Path::new("my-container");

    storage
        .create_dir(
            "clear_then_list_objects",
            "create-dir",
            namespace.clone(),
            container,
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "clear_then_list_objects",
            "put-raw-1",
            namespace.clone(),
            &container.join("obj1"),
            &Bytes::from("data1"),
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "clear_then_list_objects",
            "put-raw-2",
            namespace.clone(),
            &container.join("obj2"),
            &Bytes::from("data2"),
        )
        .await
        .unwrap();

    storage
        .delete_dir(
            "clear_then_list_objects",
            "delete-dir",
            namespace.clone(),
            container,
        )
        .await
        .unwrap();
    storage
        .create_dir(
            "clear_then_list_objects",
            "create-dir",
            namespace.clone(),
            container,
        )
        .await
        .unwrap();

    let exists = storage
        .exists(
            "clear_then_list_objects",
            "exists",
            namespace.clone(),
            container,
        )
        .await
        .unwrap();
    assert_eq!(exists, ExistsResult::Directory);

    let entries = storage
        .list_dir(
            "clear_then_list_objects",
            "list-dir",
            namespace.clone(),
            container,
        )
        .await
        .unwrap();
    assert_eq!(entries, Vec::<PathBuf>::new());
}

#[test]
#[tracing::instrument]
async fn delete_dir_root_path_is_safe_noop(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let paths = [Path::new("keep-me"), Path::new("keep/me/too")];

    for root_path in ROOT_PATHS {
        for path in paths {
            storage
                .put_raw(
                    "delete_dir_root_path_is_safe_noop",
                    "put-file",
                    namespace.clone(),
                    path,
                    &Bytes::from("payload"),
                )
                .await
                .unwrap();
        }

        let deleted = storage
            .delete_dir(
                "delete_dir_root_path_is_safe_noop",
                "delete-root-dir",
                namespace.clone(),
                Path::new(root_path),
            )
            .await
            .unwrap();

        assert!(!deleted, "delete_dir({root_path:?}) deleted a directory");

        for path in paths {
            let remaining = storage
                .get_raw(
                    "delete_dir_root_path_is_safe_noop",
                    "get-file",
                    namespace.clone(),
                    path,
                )
                .await
                .unwrap();

            assert_eq!(
                remaining,
                Some(Bytes::from("payload").to_vec()),
                "delete_dir({root_path:?}) removed {path:?}"
            );
        }
    }
}

#[test]
#[tracing::instrument]
async fn delete_dir_deletes_a_directory_that_only_holds_blobs(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let blob_path = Path::new("only/blob");

    // No create_dir: the directory `only` exists because a blob is below it.
    storage
        .put_raw(
            "delete_dir_deletes_a_directory_that_only_holds_blobs",
            "put-blob",
            namespace.clone(),
            blob_path,
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    let deleted = storage
        .delete_dir(
            "delete_dir_deletes_a_directory_that_only_holds_blobs",
            "delete-dir",
            namespace.clone(),
            Path::new("only"),
        )
        .await
        .unwrap();

    assert!(deleted);

    let remaining = storage
        .get_raw(
            "delete_dir_deletes_a_directory_that_only_holds_blobs",
            "get-blob",
            namespace.clone(),
            blob_path,
        )
        .await
        .unwrap();

    assert_eq!(remaining, None);
}

#[test]
#[tracing::instrument]
async fn delete_dir_keeps_siblings_that_differ_only_in_case(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let create_dir = |op: &'static str, path: &'static str| {
        storage.create_dir(
            "delete_dir_keeps_siblings_that_differ_only_in_case",
            op,
            namespace.clone(),
            Path::new(path),
        )
    };
    let put = |op: &'static str, path: &'static str| {
        storage.put_raw(
            "delete_dir_keeps_siblings_that_differ_only_in_case",
            op,
            namespace.clone(),
            Path::new(path),
            path.as_bytes(),
        )
    };
    let get = |op: &'static str, path: &'static str| {
        storage.get_raw(
            "delete_dir_keeps_siblings_that_differ_only_in_case",
            op,
            namespace.clone(),
            Path::new(path),
        )
    };

    create_dir("create-dir", "foo").await.unwrap();
    put("put-a", "foo/a").await.unwrap();

    // A case-insensitive store maps `FOO` onto `foo`, so only a case-sensitive store gets the
    // sibling that differs only in case.
    let case_sensitive = storage
        .exists(
            "delete_dir_keeps_siblings_that_differ_only_in_case",
            "exists",
            namespace.clone(),
            Path::new("FOO/a"),
        )
        .await
        .unwrap()
        == ExistsResult::DoesNotExist;
    if case_sensitive {
        create_dir("create-sibling-dir", "FOO").await.unwrap();
        put("put-c", "FOO/x/c").await.unwrap();
    }

    storage
        .delete_dir(
            "delete_dir_keeps_siblings_that_differ_only_in_case",
            "delete-dir",
            namespace.clone(),
            Path::new("foo"),
        )
        .await
        .unwrap();

    assert_eq!(
        get("get-c", "FOO/x/c").await.unwrap(),
        case_sensitive.then(|| b"FOO/x/c".to_vec())
    );
    assert_eq!(get("get-a", "foo/a").await.unwrap(), None);
}

#[test]
#[tracing::instrument]
async fn delete_dir_deletes_nested_descendants(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let put = |op: &'static str, path: &'static str| {
        storage.put_raw(
            "delete_dir_deletes_nested_descendants",
            op,
            namespace.clone(),
            Path::new(path),
            path.as_bytes(),
        )
    };
    let get = |op: &'static str, path: &'static str| {
        storage.get_raw(
            "delete_dir_deletes_nested_descendants",
            op,
            namespace.clone(),
            Path::new(path),
        )
    };

    storage
        .create_dir(
            "delete_dir_deletes_nested_descendants",
            "create-dir",
            namespace.clone(),
            Path::new("nested"),
        )
        .await
        .unwrap();
    put("put-direct", "nested/direct").await.unwrap();
    put("put-below", "nested/below/deep").await.unwrap();

    let deleted = storage
        .delete_dir(
            "delete_dir_deletes_nested_descendants",
            "delete-dir",
            namespace.clone(),
            Path::new("nested"),
        )
        .await
        .unwrap();

    assert!(deleted);
    assert_eq!(get("get-direct", "nested/direct").await.unwrap(), None);
    assert_eq!(get("get-below", "nested/below/deep").await.unwrap(), None);
}

#[test]
#[tracing::instrument]
async fn delete_dir_keeps_blobs_of_other_namespaces(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[tagged_as("cc")] deleted_namespace: &BlobStorageNamespace,
    #[tagged_as("cs")] kept_namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let directory = Path::new("shared");
    let blob = Path::new("shared/blob");

    storage
        .create_dir(
            "delete_dir_keeps_blobs_of_other_namespaces",
            "create-dir",
            deleted_namespace.clone(),
            directory,
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "delete_dir_keeps_blobs_of_other_namespaces",
            "put-deleted",
            deleted_namespace.clone(),
            blob,
            &Bytes::from("payload"),
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "delete_dir_keeps_blobs_of_other_namespaces",
            "put-kept",
            kept_namespace.clone(),
            blob,
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    let deleted = storage
        .delete_dir(
            "delete_dir_keeps_blobs_of_other_namespaces",
            "delete-dir",
            deleted_namespace.clone(),
            directory,
        )
        .await
        .unwrap();

    let kept = storage
        .get_raw(
            "delete_dir_keeps_blobs_of_other_namespaces",
            "get-kept",
            kept_namespace.clone(),
            blob,
        )
        .await
        .unwrap();

    assert!(deleted);
    assert_eq!(kept, Some(Bytes::from("payload").to_vec()));
}

async fn put_blobs(
    storage: &Arc<dyn BlobStorage + Send + Sync>,
    namespace: &BlobStorageNamespace,
    blobs: &[(&str, usize)],
) {
    futures::stream::iter(blobs)
        .then(|(path, size)| async move {
            storage
                .put_raw(
                    "put_blobs",
                    "put-raw",
                    namespace.clone(),
                    Path::new(path),
                    &vec![7u8; *size],
                )
                .await
        })
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
}

async fn sorted_listing(
    storage: &Arc<dyn BlobStorage + Send + Sync>,
    namespace: &BlobStorageNamespace,
    path: &str,
) -> Vec<ListedBlob> {
    let mut listed = storage
        .list_blobs_below("sorted_listing", "list", namespace.clone(), Path::new(path))
        .await
        .unwrap()
        .into_vec();
    listed.sort();
    listed
}

fn listed_blobs(blobs: &[(&str, usize)]) -> Vec<ListedBlob> {
    let mut listed = blobs
        .iter()
        .map(|(path, size)| ListedBlob {
            path: Path::new(path).into(),
            size: *size as u64,
        })
        .collect::<Vec<_>>();
    listed.sort();
    listed
}

#[test]
#[tracing::instrument]
async fn list_blobs_below_finds_nested_blobs_with_sizes(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let below_tree = [("tree/a", 1), ("tree/x/b", 2), ("tree/x/y/c", 3)];
    let sibling = [("tree2/d", 4)];
    put_blobs(&storage, namespace, &below_tree).await;
    put_blobs(&storage, namespace, &sibling).await;
    storage
        .create_dir(
            "list_blobs_below_finds_nested_blobs_with_sizes",
            "create-dir",
            namespace.clone(),
            Path::new("tree/empty"),
        )
        .await
        .unwrap();
    // A case-insensitive store maps `TREE` onto `tree`, so only a case-sensitive store gets the
    // sibling that differs only in case.
    let case_sensitive = storage
        .exists(
            "list_blobs_below_finds_nested_blobs_with_sizes",
            "exists",
            namespace.clone(),
            Path::new("TREE/a"),
        )
        .await
        .unwrap()
        == ExistsResult::DoesNotExist;
    let case_sibling: &[(&str, usize)] = if case_sensitive {
        &[("TREE/x/e", 5)]
    } else {
        &[]
    };
    put_blobs(&storage, namespace, case_sibling).await;

    let listed_below_tree = sorted_listing(&storage, namespace, "tree").await;
    let listed_below_root = sorted_listing(&storage, namespace, "").await;
    let listed_below_missing = sorted_listing(&storage, namespace, "missing").await;
    let listed_below_blob = sorted_listing(&storage, namespace, "tree/a").await;

    assert_eq!(listed_below_tree, listed_blobs(&below_tree));
    assert_eq!(
        listed_below_root,
        listed_blobs(&[&below_tree[..], &sibling, case_sibling].concat())
    );
    assert_eq!(listed_below_missing, Vec::new());
    assert_eq!(listed_below_blob, Vec::new());
}

/// The namespace of the same kind in another environment.
fn in_another_environment(namespace: &BlobStorageNamespace) -> BlobStorageNamespace {
    let environment_id = EnvironmentId(Uuid::new_v4());
    match namespace.clone() {
        BlobStorageNamespace::CompilationCache { .. } => {
            BlobStorageNamespace::CompilationCache { environment_id }
        }
        BlobStorageNamespace::InitialAgentFiles { .. } => {
            BlobStorageNamespace::InitialAgentFiles { environment_id }
        }
        BlobStorageNamespace::CustomStorage { .. } => {
            BlobStorageNamespace::CustomStorage { environment_id }
        }
        BlobStorageNamespace::OplogPayload {
            agent_id,
            agent_mode,
            ..
        } => BlobStorageNamespace::OplogPayload {
            environment_id,
            agent_id,
            agent_mode,
        },
        BlobStorageNamespace::CompressedOplog {
            component_id,
            agent_mode,
            level,
            ..
        } => BlobStorageNamespace::CompressedOplog {
            environment_id,
            component_id,
            agent_mode,
            level,
        },
        BlobStorageNamespace::Components { .. } => {
            BlobStorageNamespace::Components { environment_id }
        }
        BlobStorageNamespace::FilesystemSnapshots { agent_id, .. } => {
            BlobStorageNamespace::FilesystemSnapshots {
                environment_id,
                agent_id,
            }
        }
    }
}

#[test]
#[tracing::instrument]
async fn list_blobs_below_stays_in_its_namespace(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let other_namespace = in_another_environment(namespace);
    let blobs = [("tree/a", 1)];
    let other_blobs = [("tree/b", 2)];
    put_blobs(&storage, namespace, &blobs).await;
    put_blobs(&storage, &other_namespace, &other_blobs).await;

    let listed_below_tree = sorted_listing(&storage, namespace, "tree").await;
    let listed_below_root = sorted_listing(&storage, namespace, "").await;
    let other_listed_below_tree = sorted_listing(&storage, &other_namespace, "tree").await;

    assert_eq!(listed_below_tree, listed_blobs(&blobs));
    assert_eq!(listed_below_root, listed_blobs(&blobs));
    assert_eq!(other_listed_below_tree, listed_blobs(&other_blobs));
}

#[test]
#[tracing::instrument]
async fn get_raw_slice_uses_inclusive_ranges(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    put_blobs(&storage, namespace, &[("ranges/empty", 0)]).await;
    storage
        .put_raw(
            "get_raw_slice_uses_inclusive_ranges",
            "put-raw",
            namespace.clone(),
            Path::new("ranges/blob"),
            b"abcdef",
        )
        .await
        .unwrap();
    let read = |path: &'static str, start: u64, end: u64| {
        storage.get_raw_slice(
            "get_raw_slice_uses_inclusive_ranges",
            "get-raw-slice",
            namespace.clone(),
            Path::new(path),
            start,
            end,
        )
    };

    assert_eq!(
        read("ranges/blob", 1, 3).await.unwrap(),
        Some(b"bcd".to_vec())
    );
    assert_eq!(
        read("ranges/blob", 0, 5).await.unwrap(),
        Some(b"abcdef".to_vec())
    );
    assert_eq!(
        read("ranges/blob", 5, 5).await.unwrap(),
        Some(b"f".to_vec())
    );
    assert_eq!(read("ranges/missing", 0, 0).await.unwrap(), None);

    let outside = [
        ("ranges/blob", 0, 6),
        ("ranges/blob", 6, 6),
        ("ranges/blob", 3, 2),
        ("ranges/empty", 0, 0),
        ("ranges/missing", 3, 2),
        // A guest gives an offset as a `u64`. A negative offset reaches the host as the
        // value that it wraps to, which is at the top of the `u64` range. The Effect SDK
        // test doubles assert on these two ranges (`sdks/effect/test/blobstore.test.ts`).
        // The wrapped start is a start after the end, which each backend rejects as it
        // rejects `(3, 2)`, before the S3 backend sends a request. The wrapped end reaches
        // the backend. The MinIO release that `S3Test` starts parses each offset of the
        // range with `strconv.ParseInt(s, 10, 64)` in `parseRequestRangeSpec`
        // (`cmd/httprange.go`), so 2^64-1 overflows `int64` and gives a parse error that
        // is not `errInvalidRange`. `getObjectHandler` (`cmd/object-handlers.go`) ignores
        // such a parse error and serves a regular GET: the status 200, the full object and
        // no `Content-Range`. This is the integer overflow path of MinIO, not a behaviour
        // of S3.
        ("ranges/blob", u64::MAX, 2),
        ("ranges/blob", u64::MAX, u64::MAX),
    ];
    let range_errors = futures::stream::iter(outside)
        .then(|(path, start, end)| async move {
            let error = read(path, start, end).await.err();
            (
                path,
                error.and_then(|error| error.downcast_ref::<BlobRangeError>().copied()),
            )
        })
        .collect::<Vec<_>>()
        .await;

    assert_eq!(
        range_errors,
        outside
            .map(|(path, start, end)| (path, Some(BlobRangeError { start, end })))
            .to_vec()
    );
}

fn bulk_paths() -> Vec<PathBuf> {
    (0..2001)
        .map(|index| PathBuf::from(format!("bulk/{index:04}")))
        .collect()
}

async fn put_bulk_blobs(
    storage: &Arc<dyn BlobStorage + Send + Sync>,
    namespace: &BlobStorageNamespace,
    paths: &[PathBuf],
) {
    futures::stream::iter(paths.iter().cloned())
        .map(|path| async move {
            storage
                .put_raw(
                    "put_bulk_blobs",
                    "put-raw",
                    namespace.clone(),
                    &path,
                    b"bulk",
                )
                .await
        })
        .buffer_unordered(32)
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
}

#[test]
#[tracing::instrument]
async fn delete_many_deletes_more_than_1000_blobs(
    #[dimension(s3_storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[tagged_as("cs")] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let paths = bulk_paths();
    put_bulk_blobs(&storage, namespace, &paths).await;
    let written = sorted_listing(&storage, namespace, "bulk").await.len();

    storage
        .delete_many(
            "delete_many_deletes_more_than_1000_blobs",
            "delete-many",
            namespace.clone(),
            &paths,
        )
        .await
        .unwrap();

    assert_eq!(
        (written, sorted_listing(&storage, namespace, "bulk").await),
        (2001, Vec::new())
    );
}

#[test]
#[tracing::instrument]
async fn delete_dir_deletes_more_than_1000_blobs(
    #[dimension(s3_storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[tagged_as("cs")] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    put_bulk_blobs(&storage, namespace, &bulk_paths()).await;
    let written = sorted_listing(&storage, namespace, "bulk").await.len();

    let deleted = storage
        .delete_dir(
            "delete_dir_deletes_more_than_1000_blobs",
            "delete-dir",
            namespace.clone(),
            Path::new("bulk"),
        )
        .await
        .unwrap();

    assert_eq!(
        (
            written,
            deleted,
            sorted_listing(&storage, namespace, "bulk").await
        ),
        (2001, true, Vec::new())
    );
}

#[test]
#[tracing::instrument]
async fn a_leading_current_dir_names_the_same_blob(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let dotted = Path::new("./leading-dot-blob");
    let plain = Path::new("leading-dot-blob");
    let data = Bytes::from("leading-dot-payload").to_vec();

    storage
        .put_raw(
            "a_leading_current_dir_names_the_same_blob",
            "put-dotted",
            namespace.clone(),
            dotted,
            &data,
        )
        .await
        .unwrap();

    assert_eq!(
        storage
            .get_raw(
                "a_leading_current_dir_names_the_same_blob",
                "get-plain",
                namespace.clone(),
                plain,
            )
            .await
            .unwrap(),
        Some(data.clone())
    );
    assert_eq!(
        storage
            .get_raw(
                "a_leading_current_dir_names_the_same_blob",
                "get-dotted",
                namespace.clone(),
                dotted,
            )
            .await
            .unwrap(),
        Some(data.clone())
    );
    assert_eq!(
        storage
            .exists(
                "a_leading_current_dir_names_the_same_blob",
                "exists-plain",
                namespace.clone(),
                plain,
            )
            .await
            .unwrap(),
        ExistsResult::File
    );
    assert_eq!(
        storage
            .exists(
                "a_leading_current_dir_names_the_same_blob",
                "exists-dotted",
                namespace.clone(),
                dotted,
            )
            .await
            .unwrap(),
        ExistsResult::File
    );

    storage
        .delete(
            "a_leading_current_dir_names_the_same_blob",
            "delete-dotted",
            namespace.clone(),
            dotted,
        )
        .await
        .unwrap();

    assert_eq!(
        storage
            .get_raw(
                "a_leading_current_dir_names_the_same_blob",
                "get-deleted",
                namespace.clone(),
                plain,
            )
            .await
            .unwrap(),
        None
    );
}

#[test]
#[tracing::instrument]
async fn an_interior_current_dir_names_the_same_blob(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let dotted = Path::new("interior-dot-dir/./blob");
    let plain = Path::new("interior-dot-dir/blob");
    let data = Bytes::from("interior-dot-payload").to_vec();

    storage
        .put_raw(
            "an_interior_current_dir_names_the_same_blob",
            "put-dotted",
            namespace.clone(),
            dotted,
            &data,
        )
        .await
        .unwrap();

    assert_eq!(
        storage
            .get_raw(
                "an_interior_current_dir_names_the_same_blob",
                "get-plain",
                namespace.clone(),
                plain,
            )
            .await
            .unwrap(),
        Some(data.clone())
    );
    assert_eq!(
        storage
            .exists(
                "an_interior_current_dir_names_the_same_blob",
                "exists-dotted",
                namespace.clone(),
                dotted,
            )
            .await
            .unwrap(),
        ExistsResult::File
    );
    // A backend that keeps no entry for a directory that only holds blobs reports that directory
    // as missing. The two spellings of the directory must still agree.
    assert_eq!(
        storage
            .exists(
                "an_interior_current_dir_names_the_same_blob",
                "exists-dotted-dir",
                namespace.clone(),
                Path::new("interior-dot-dir/."),
            )
            .await
            .unwrap(),
        storage
            .exists(
                "an_interior_current_dir_names_the_same_blob",
                "exists-plain-dir",
                namespace.clone(),
                Path::new("interior-dot-dir"),
            )
            .await
            .unwrap(),
    );

    storage
        .delete(
            "an_interior_current_dir_names_the_same_blob",
            "delete-dotted",
            namespace.clone(),
            dotted,
        )
        .await
        .unwrap();

    assert_eq!(
        storage
            .get_raw(
                "an_interior_current_dir_names_the_same_blob",
                "get-deleted",
                namespace.clone(),
                plain,
            )
            .await
            .unwrap(),
        None
    );
}

#[test]
#[tracing::instrument]
async fn a_repeated_separator_names_the_same_blob(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let doubled = Path::new("repeated-sep-dir//blob");
    let plain = Path::new("repeated-sep-dir/blob");
    let data = Bytes::from("repeated-sep-payload").to_vec();

    storage
        .put_raw(
            "a_repeated_separator_names_the_same_blob",
            "put-doubled",
            namespace.clone(),
            doubled,
            &data,
        )
        .await
        .unwrap();

    assert_eq!(
        storage
            .get_raw(
                "a_repeated_separator_names_the_same_blob",
                "get-plain",
                namespace.clone(),
                plain,
            )
            .await
            .unwrap(),
        Some(data.clone())
    );
    assert_eq!(
        storage
            .exists(
                "a_repeated_separator_names_the_same_blob",
                "exists-doubled",
                namespace.clone(),
                doubled,
            )
            .await
            .unwrap(),
        ExistsResult::File
    );

    storage
        .delete(
            "a_repeated_separator_names_the_same_blob",
            "delete-doubled",
            namespace.clone(),
            doubled,
        )
        .await
        .unwrap();

    assert_eq!(
        storage
            .get_raw(
                "a_repeated_separator_names_the_same_blob",
                "get-deleted",
                namespace.clone(),
                plain,
            )
            .await
            .unwrap(),
        None
    );
}

#[test]
#[tracing::instrument]
async fn create_dir_at_a_root_path_leaves_nothing_behind(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
) {
    let storage = test.get_blob_storage().await;
    // The namespace is fresh, so a stray entry at its root can only come from this test. The S3
    // backend keeps one bucket for every environment, so a shared namespace would hide one.
    let namespace = BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId(Uuid::new_v4()),
    };

    // One blob is at the root, so the root is there to list on every backend, and the listing
    // below has one thing that it must hold.
    storage
        .put_raw(
            "create_dir_at_a_root_path_leaves_nothing_behind",
            "put-blob",
            namespace.clone(),
            Path::new("only-blob"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    for root_path in ROOT_PATHS {
        storage
            .create_dir(
                "create_dir_at_a_root_path_leaves_nothing_behind",
                "create-root-dir",
                namespace.clone(),
                Path::new(root_path),
            )
            .await
            .unwrap_or_else(|err| panic!("create_dir({root_path:?}) failed: {err}"));

        // The listing of the root holds the one blob and nothing more. The S3 backend records a
        // directory with an object named `__dir_marker` inside it, and the name is now one that
        // the backend keeps for itself, so this test cannot read such an object back by its
        // name. The S3 unit test `create_dir_at_a_root_path_sends_no_request` states the same
        // thing for that backend, on the request that `create_dir` would have to send.
        let mut entries = storage
            .list_dir(
                "create_dir_at_a_root_path_leaves_nothing_behind",
                "list-root",
                namespace.clone(),
                Path::new(""),
            )
            .await
            .unwrap();
        entries.sort();

        assert_eq!(
            entries,
            vec![Path::new("only-blob").to_path_buf()],
            "create_dir({root_path:?}) left something at the root"
        );
    }
}

#[test]
#[tracing::instrument]
async fn exists_at_a_root_path_gives_a_directory(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    for root_path in ROOT_PATHS {
        let before = storage
            .exists(
                "exists_at_a_root_path_gives_a_directory",
                "exists-before",
                namespace.clone(),
                Path::new(root_path),
            )
            .await
            .unwrap();

        assert_eq!(
            before,
            ExistsResult::Directory,
            "exists({root_path:?}) before a write"
        );
    }

    storage
        .put_raw(
            "exists_at_a_root_path_gives_a_directory",
            "put-blob",
            namespace.clone(),
            Path::new("blob"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    for root_path in ROOT_PATHS {
        let after = storage
            .exists(
                "exists_at_a_root_path_gives_a_directory",
                "exists-after",
                namespace.clone(),
                Path::new(root_path),
            )
            .await
            .unwrap();

        assert_eq!(
            after,
            ExistsResult::Directory,
            "exists({root_path:?}) after a write"
        );
    }
}

#[test]
#[tracing::instrument]
async fn exists_on_a_directory_that_only_holds_blobs(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    // No create_dir: these directories exist because blobs are below them.
    for blob in [Path::new("only/blob"), Path::new("deep/nested/dirs/blob")] {
        storage
            .put_raw(
                "exists_on_a_directory_that_only_holds_blobs",
                "put-blob",
                namespace.clone(),
                blob,
                &Bytes::from("payload"),
            )
            .await
            .unwrap();
    }

    for directory in [
        Path::new("only"),
        Path::new("deep"),
        Path::new("deep/nested"),
        Path::new("deep/nested/dirs"),
    ] {
        let result = storage
            .exists(
                "exists_on_a_directory_that_only_holds_blobs",
                "exists-dir",
                namespace.clone(),
                directory,
            )
            .await
            .unwrap();

        assert_eq!(result, ExistsResult::Directory, "exists({directory:?})");
    }

    for blob in [Path::new("only/blob"), Path::new("deep/nested/dirs/blob")] {
        let result = storage
            .exists(
                "exists_on_a_directory_that_only_holds_blobs",
                "exists-blob",
                namespace.clone(),
                blob,
            )
            .await
            .unwrap();

        assert_eq!(result, ExistsResult::File, "exists({blob:?})");
    }

    let missing = storage
        .exists(
            "exists_on_a_directory_that_only_holds_blobs",
            "exists-missing",
            namespace.clone(),
            Path::new("deep/nested/other"),
        )
        .await
        .unwrap();

    assert_eq!(missing, ExistsResult::DoesNotExist);
}

#[test]
#[tracing::instrument]
async fn exists_on_a_blob_gives_a_file(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let blob = Path::new("not-a-dir");

    storage
        .put_raw(
            "exists_on_a_blob_gives_a_file",
            "put-blob",
            namespace.clone(),
            blob,
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    let result = storage
        .exists(
            "exists_on_a_blob_gives_a_file",
            "exists-blob",
            namespace.clone(),
            blob,
        )
        .await
        .unwrap();

    let missing = storage
        .exists(
            "exists_on_a_blob_gives_a_file",
            "exists-missing",
            namespace.clone(),
            Path::new("not-a-dir-either"),
        )
        .await
        .unwrap();

    assert_eq!(result, ExistsResult::File);
    assert_eq!(missing, ExistsResult::DoesNotExist);
}

#[test]
#[tracing::instrument]
async fn list_dir_of_a_path_with_nothing_gives_an_empty_list(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let empty: Vec<PathBuf> = Vec::new();

    // Nothing is written yet, so the root of the namespace holds nothing.
    for root_path in ROOT_PATHS {
        let entries = storage
            .list_dir(
                "list_dir_of_a_path_with_nothing_gives_an_empty_list",
                "list-root",
                namespace.clone(),
                Path::new(root_path),
            )
            .await
            .unwrap();

        assert_eq!(entries, empty, "list_dir({root_path:?}) before a write");
    }

    storage
        .put_raw(
            "list_dir_of_a_path_with_nothing_gives_an_empty_list",
            "put-blob",
            namespace.clone(),
            Path::new("blob"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    let entries = storage
        .list_dir(
            "list_dir_of_a_path_with_nothing_gives_an_empty_list",
            "list-missing-dir",
            namespace.clone(),
            Path::new("no-such-dir"),
        )
        .await
        .unwrap();

    assert_eq!(entries, empty, "list_dir(\"no-such-dir\")");
}

#[test]
#[tracing::instrument]
async fn exists_on_a_blob_that_also_has_blobs_below_gives_a_file(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "exists_on_a_blob_that_also_has_blobs_below_gives_a_file";

    storage
        .put_raw(
            label,
            "put-blob",
            namespace.clone(),
            Path::new("a"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    // A blob and a directory cannot share one path on the filesystem backend, which refuses
    // this write, so the rest of the case belongs to the other backends.
    if storage
        .put_raw(
            label,
            "put-below",
            namespace.clone(),
            Path::new("a/b"),
            &Bytes::from("payload"),
        )
        .await
        .is_err()
    {
        return;
    }

    assert_eq!(
        storage
            .exists(label, "exists-blob", namespace.clone(), Path::new("a"))
            .await
            .unwrap(),
        ExistsResult::File
    );
}

#[test]
#[tracing::instrument]
async fn a_read_at_a_root_path_finds_no_blob(
    #[dimension(mem_and_s3)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "a_read_at_a_root_path_finds_no_blob";

    // A blob at the root of the namespace must not answer for the root itself.
    storage
        .put_raw(
            label,
            "put-blob",
            namespace.clone(),
            Path::new("blob"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    for root_path in ROOT_PATHS {
        let path = Path::new(root_path);

        assert_eq!(
            storage
                .get_raw(label, "get-raw", namespace.clone(), path)
                .await
                .unwrap(),
            None,
            "get_raw({root_path:?})"
        );

        assert!(
            storage
                .get_stream(label, "get-stream", namespace.clone(), path)
                .await
                .unwrap()
                .is_none(),
            "get_stream({root_path:?})"
        );

        assert!(
            storage
                .get_metadata(label, "get-metadata", namespace.clone(), path)
                .await
                .unwrap()
                .is_none(),
            "get_metadata({root_path:?})"
        );

        assert_eq!(
            storage
                .get_raw_slice(label, "get-raw-slice", namespace.clone(), path, 0, 1)
                .await
                .unwrap(),
            None,
            "get_raw_slice({root_path:?})"
        );
    }
}

#[test]
#[tracing::instrument]
async fn a_write_at_a_root_path_is_an_error(
    #[dimension(mem_and_s3)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "a_write_at_a_root_path_is_an_error";
    let data = Bytes::from("payload").to_vec();

    for root_path in ROOT_PATHS {
        let path = Path::new(root_path);

        assert!(
            storage
                .put_raw(label, "put-raw", namespace.clone(), path, &data)
                .await
                .is_err(),
            "put_raw({root_path:?}) wrote a blob"
        );

        let stream = (&data)
            .map_item(|i| i.map_err(widen_infallible))
            .map_error(widen_infallible)
            .erased();

        assert!(
            storage
                .put_stream(label, "put-stream", namespace.clone(), path, &stream)
                .await
                .is_err(),
            "put_stream({root_path:?}) wrote a blob"
        );

        assert_eq!(
            storage
                .get_raw(label, "get-raw", namespace.clone(), path)
                .await
                .unwrap(),
            None,
            "a write at {root_path:?} left a blob behind"
        );
    }

    assert_eq!(
        storage
            .list_dir(label, "list-root", namespace.clone(), Path::new(""))
            .await
            .unwrap(),
        Vec::<PathBuf>::new()
    );
}

#[test]
#[tracing::instrument]
async fn a_delete_at_a_root_path_deletes_no_blob(
    #[dimension(mem_and_s3)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "a_delete_at_a_root_path_deletes_no_blob";

    storage
        .put_raw(
            label,
            "put-blob",
            namespace.clone(),
            Path::new("blob"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    for root_path in ROOT_PATHS {
        storage
            .delete(label, "delete", namespace.clone(), Path::new(root_path))
            .await
            .unwrap_or_else(|err| panic!("delete({root_path:?}) failed: {err}"));

        storage
            .delete_many(
                label,
                "delete-many",
                namespace.clone(),
                &[PathBuf::from(root_path)],
            )
            .await
            .unwrap_or_else(|err| panic!("delete_many([{root_path:?}]) failed: {err}"));

        assert_eq!(
            storage
                .get_raw(label, "get-blob", namespace.clone(), Path::new("blob"))
                .await
                .unwrap(),
            Some(Bytes::from("payload").to_vec()),
            "a delete at {root_path:?} removed a blob"
        );
    }
}

#[test]
#[tracing::instrument]
async fn a_copy_or_a_move_at_a_root_path_is_an_error(
    #[dimension(mem_and_s3)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "a_copy_or_a_move_at_a_root_path_is_an_error";

    storage
        .put_raw(
            label,
            "put-blob",
            namespace.clone(),
            Path::new("blob"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    for root_path in ROOT_PATHS {
        for (from, to) in [(root_path, "blob"), ("blob", root_path)] {
            assert!(
                storage
                    .copy(
                        label,
                        "copy",
                        namespace.clone(),
                        Path::new(from),
                        Path::new(to)
                    )
                    .await
                    .is_err(),
                "copy({from:?}, {to:?})"
            );

            assert!(
                storage
                    .r#move(
                        label,
                        "move",
                        namespace.clone(),
                        Path::new(from),
                        Path::new(to)
                    )
                    .await
                    .is_err(),
                "move({from:?}, {to:?})"
            );
        }

        assert_eq!(
            storage
                .get_raw(label, "get-blob", namespace.clone(), Path::new("blob"))
                .await
                .unwrap(),
            Some(Bytes::from("payload").to_vec()),
            "a copy or a move at {root_path:?} changed the blob"
        );

        assert_eq!(
            storage
                .get_raw(label, "get-root", namespace.clone(), Path::new(root_path))
                .await
                .unwrap(),
            None,
            "a copy or a move at {root_path:?} left a blob behind"
        );
    }
}

#[test]
#[tracing::instrument]
async fn a_directory_of_blobs_goes_when_its_last_blob_goes(
    #[dimension(mem_and_s3)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "a_directory_of_blobs_goes_when_its_last_blob_goes";

    // No create_dir: the directory `only` exists because a blob is below it.
    storage
        .put_raw(
            label,
            "put-blob",
            namespace.clone(),
            Path::new("only/blob"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    storage
        .delete(
            label,
            "delete-blob",
            namespace.clone(),
            Path::new("only/blob"),
        )
        .await
        .unwrap();

    assert_eq!(
        storage
            .exists(label, "exists-only", namespace.clone(), Path::new("only"))
            .await
            .unwrap(),
        ExistsResult::DoesNotExist
    );

    assert!(
        !storage
            .delete_dir(label, "delete-only", namespace.clone(), Path::new("only"))
            .await
            .unwrap()
    );

    // A directory that create_dir made stays after its last blob goes.
    storage
        .create_dir(label, "create-kept", namespace.clone(), Path::new("kept"))
        .await
        .unwrap();

    storage
        .put_raw(
            label,
            "put-kept-blob",
            namespace.clone(),
            Path::new("kept/blob"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    storage
        .delete(
            label,
            "delete-kept-blob",
            namespace.clone(),
            Path::new("kept/blob"),
        )
        .await
        .unwrap();

    assert_eq!(
        storage
            .exists(label, "exists-kept", namespace.clone(), Path::new("kept"))
            .await
            .unwrap(),
        ExistsResult::Directory
    );

    assert!(
        storage
            .delete_dir(label, "delete-kept", namespace.clone(), Path::new("kept"))
            .await
            .unwrap()
    );
}

#[test]
#[tracing::instrument]
async fn delete_dir_deletes_a_directory_further_up_that_only_holds_blobs(
    #[dimension(mem_and_s3)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "delete_dir_deletes_a_directory_further_up_that_only_holds_blobs";
    let blob_path = Path::new("a/b/c/blob");

    // No create_dir: the directories `a`, `a/b` and `a/b/c` exist because a blob is below them.
    storage
        .put_raw(
            label,
            "put-blob",
            namespace.clone(),
            blob_path,
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    assert!(
        storage
            .delete_dir(label, "delete-dir", namespace.clone(), Path::new("a"))
            .await
            .unwrap()
    );

    assert_eq!(
        storage
            .get_raw(label, "get-blob", namespace.clone(), blob_path)
            .await
            .unwrap(),
        None
    );

    assert_eq!(
        storage
            .exists(label, "exists-dir", namespace.clone(), Path::new("a"))
            .await
            .unwrap(),
        ExistsResult::DoesNotExist
    );
}

#[test]
#[tracing::instrument]
async fn get_metadata_of_a_created_directory_gives_metadata(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "get_metadata_of_a_created_directory_gives_metadata";

    storage
        .create_dir(label, "create-dir", namespace.clone(), Path::new("made"))
        .await
        .unwrap();

    // A container of the blob store gets its time from here, so a directory that create_dir made
    // has metadata.
    assert!(
        storage
            .get_metadata(
                label,
                "get-metadata-dir",
                namespace.clone(),
                Path::new("made")
            )
            .await
            .unwrap()
            .is_some()
    );
}

#[test]
#[tracing::instrument]
async fn get_metadata_of_a_created_directory_gives_a_size_of_zero(
    #[dimension(mem_and_s3)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "get_metadata_of_a_created_directory_gives_a_size_of_zero";

    storage
        .create_dir(label, "create-dir", namespace.clone(), Path::new("made"))
        .await
        .unwrap();

    assert_eq!(
        storage
            .get_metadata(
                label,
                "get-metadata-dir",
                namespace.clone(),
                Path::new("made")
            )
            .await
            .unwrap()
            .unwrap()
            .size,
        0
    );
}

#[test]
#[tracing::instrument]
async fn get_metadata_of_a_directory_of_blobs_finds_no_blob(
    #[dimension(mem_and_s3)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "get_metadata_of_a_directory_of_blobs_finds_no_blob";

    // No create_dir: the directory `implied` exists because a blob is below it.
    storage
        .put_raw(
            label,
            "put-blob",
            namespace.clone(),
            Path::new("implied/blob"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    assert!(
        storage
            .get_metadata(
                label,
                "get-metadata-dir",
                namespace.clone(),
                Path::new("implied")
            )
            .await
            .unwrap()
            .is_none()
    );

    assert_eq!(
        storage
            .get_metadata(
                label,
                "get-metadata-blob",
                namespace.clone(),
                Path::new("implied/blob")
            )
            .await
            .unwrap()
            .unwrap()
            .size,
        "payload".len() as u64
    );
}

#[test]
#[tracing::instrument]
async fn a_copy_or_a_move_onto_itself_changes_nothing(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "a_copy_or_a_move_onto_itself_changes_nothing";

    storage
        .put_raw(
            label,
            "put-blob",
            namespace.clone(),
            Path::new("dir/blob"),
            &Bytes::from("payload"),
        )
        .await
        .unwrap();

    // Two forms of one path name the same blob.
    for (from, to) in [
        ("dir/blob", "dir/blob"),
        ("dir/blob", "./dir/blob"),
        ("./dir/blob", "dir/blob"),
    ] {
        storage
            .copy(
                label,
                "copy",
                namespace.clone(),
                Path::new(from),
                Path::new(to),
            )
            .await
            .unwrap_or_else(|err| panic!("copy({from:?}, {to:?}) failed: {err}"));

        storage
            .r#move(
                label,
                "move",
                namespace.clone(),
                Path::new(from),
                Path::new(to),
            )
            .await
            .unwrap_or_else(|err| panic!("move({from:?}, {to:?}) failed: {err}"));

        assert_eq!(
            storage
                .get_raw(label, "get-blob", namespace.clone(), Path::new("dir/blob"))
                .await
                .unwrap(),
            Some(Bytes::from("payload").to_vec()),
            "copy({from:?}, {to:?}) or move({from:?}, {to:?}) changed the blob"
        );
    }
}

#[test]
#[tracing::instrument]
async fn a_copy_or_a_move_onto_itself_needs_a_blob(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "a_copy_or_a_move_onto_itself_needs_a_blob";

    // Two forms of one path name the same blob.
    for (from, to) in [
        ("missing/blob", "missing/blob"),
        ("./missing/blob", "missing/blob"),
    ] {
        assert!(
            storage
                .copy(
                    label,
                    "copy",
                    namespace.clone(),
                    Path::new(from),
                    Path::new(to)
                )
                .await
                .is_err(),
            "copy({from:?}, {to:?})"
        );

        assert!(
            storage
                .r#move(
                    label,
                    "move",
                    namespace.clone(),
                    Path::new(from),
                    Path::new(to)
                )
                .await
                .is_err(),
            "move({from:?}, {to:?})"
        );
    }
}

#[test]
#[tracing::instrument]
async fn put_raw_if_absent_writes_a_blob_where_the_path_has_none(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[tagged_as("fss")] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "put_raw_if_absent_writes_a_blob_where_the_path_has_none";
    let path = Path::new("dir/blob");

    let written = storage
        .put_raw_if_absent(label, "put-if-absent", namespace.clone(), path, b"first")
        .await
        .unwrap();
    let read = storage
        .get_raw(label, "get-raw", namespace.clone(), path)
        .await
        .unwrap();

    assert_eq!(
        (written, read),
        (PutIfAbsent::Written, Some(b"first".to_vec()))
    );
}

#[test]
#[tracing::instrument]
async fn put_raw_if_absent_keeps_the_blob_that_is_at_the_path(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[tagged_as("fss")] namespace: &BlobStorageNamespace,
) {
    // One blob comes from `put_raw` and one from `put_raw_if_absent`. The call refuses both, and
    // each keeps its first bytes.
    let storage = test.get_blob_storage().await;
    let label = "put_raw_if_absent_keeps_the_blob_that_is_at_the_path";
    let put = Path::new("put");
    let conditional = Path::new("dir/conditional");
    storage
        .put_raw(label, "put-raw", namespace.clone(), put, b"first")
        .await
        .unwrap();
    let first_conditional = storage
        .put_raw_if_absent(
            label,
            "put-if-absent",
            namespace.clone(),
            conditional,
            b"first",
        )
        .await
        .unwrap();

    let second_put = storage
        .put_raw_if_absent(label, "put-if-absent", namespace.clone(), put, b"second")
        .await
        .unwrap();
    let second_conditional = storage
        .put_raw_if_absent(
            label,
            "put-if-absent",
            namespace.clone(),
            conditional,
            b"second",
        )
        .await
        .unwrap();
    let read_put = storage
        .get_raw(label, "get-raw", namespace.clone(), put)
        .await
        .unwrap();
    let read_conditional = storage
        .get_raw(label, "get-raw", namespace.clone(), conditional)
        .await
        .unwrap();

    assert_eq!(
        (
            first_conditional,
            second_put,
            second_conditional,
            read_put,
            read_conditional
        ),
        (
            PutIfAbsent::Written,
            PutIfAbsent::AlreadyExists,
            PutIfAbsent::AlreadyExists,
            Some(b"first".to_vec()),
            Some(b"first".to_vec())
        )
    );
}

#[test]
#[tracing::instrument]
async fn put_raw_if_absent_writes_again_after_a_delete(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[tagged_as("fss")] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;
    let label = "put_raw_if_absent_writes_again_after_a_delete";
    let path = Path::new("blob");
    let first = storage
        .put_raw_if_absent(label, "put-if-absent", namespace.clone(), path, b"first")
        .await
        .unwrap();
    storage
        .delete(label, "delete", namespace.clone(), path)
        .await
        .unwrap();

    let second = storage
        .put_raw_if_absent(label, "put-if-absent", namespace.clone(), path, b"second")
        .await
        .unwrap();
    let read = storage
        .get_raw(label, "get-raw", namespace.clone(), path)
        .await
        .unwrap();

    assert_eq!(
        (first, second, read),
        (
            PutIfAbsent::Written,
            PutIfAbsent::Written,
            Some(b"second".to_vec())
        )
    );
}

#[test]
#[tracing::instrument]
async fn of_concurrent_put_raw_if_absent_calls_on_one_path_one_writes(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[tagged_as("fss")] namespace: &BlobStorageNamespace,
) {
    // Each call writes other bytes, so the blob tells which call wrote it. The calls run at the
    // same time. The filesystem backend writes on blocking threads, the SQLite pool has more than
    // one connection, and S3 gets the requests in parallel.
    let storage = test.get_blob_storage().await;
    let label = "of_concurrent_put_raw_if_absent_calls_on_one_path_one_writes";
    let path = Path::new("dir/blob");
    let payloads = (0..16)
        .map(|writer| format!("writer {writer}").into_bytes())
        .collect::<Vec<_>>();

    let results = futures::future::join_all(payloads.iter().map(|payload| {
        storage.put_raw_if_absent(label, "put-if-absent", namespace.clone(), path, payload)
    }))
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();
    let read = storage
        .get_raw(label, "get-raw", namespace.clone(), path)
        .await
        .unwrap();
    let writers = results
        .iter()
        .zip(&payloads)
        .filter(|(result, _)| **result == PutIfAbsent::Written)
        .map(|(_, payload)| payload.clone())
        .collect::<Vec<_>>();

    assert_eq!(
        (writers.len(), read.map(|read| writers.contains(&read))),
        (1, Some(true)),
        "{results:?}"
    );
}

#[test]
#[tracing::instrument]
async fn put_raw_if_absent_at_a_root_path_gives_the_no_name_error(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[tagged_as("fss")] namespace: &BlobStorageNamespace,
) {
    // A root path is a directory, and a blob cannot be where a directory is. Each backend gives
    // the error of the name, which is permanent, before it writes anything.
    let storage = test.get_blob_storage().await;
    let label = "put_raw_if_absent_at_a_root_path_gives_the_no_name_error";
    let storage = &storage;

    let errors = futures::future::join_all(ROOT_PATHS.map(|root_path| async move {
        storage
            .put_raw_if_absent(
                label,
                "put-if-absent",
                namespace.clone(),
                Path::new(root_path),
                b"payload",
            )
            .await
            .err()
            .and_then(name_error)
    }))
    .await;

    assert_eq!(
        errors,
        ROOT_PATHS.map(|_| Some(BlobNameError::NoName {
            path: PathBuf::new()
        }))
    );
}

#[test]
#[tracing::instrument]
async fn fs_put_raw_if_absent_gives_the_error_of_a_name_that_the_filesystem_refuses(
    #[tagged_as("fs")] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[tagged_as("fss")] namespace: &BlobStorageNamespace,
) {
    // The filesystem that holds the storage must not accept a name of 300 bytes. So the move of the
    // written file into place fails with an error that is not "already exists". The call gives that
    // error and writes no blob.
    let storage = test.get_blob_storage().await;
    let label = "fs_put_raw_if_absent_gives_the_error_of_a_name_that_the_filesystem_refuses";
    let too_long = "x".repeat(300);

    let written = storage
        .put_raw_if_absent(
            label,
            "put-if-absent",
            namespace.clone(),
            Path::new(&too_long),
            b"payload",
        )
        .await;
    let listed = sorted_listing(&storage, namespace, "").await;

    assert!(written.is_err(), "{written:?}");
    assert_eq!(listed, Vec::new());
}

#[test]
#[tracing::instrument]
async fn the_filesystem_snapshots_namespace_gives_each_agent_its_own_location(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
) {
    // Each namespace holds a blob at the same path with its own bytes. The agents differ in their
    // name, their environment or their component. The last three namespaces are other kinds of
    // namespace of the same environment. One agent name holds a `..` segment, which the rules of a
    // blob name refuse. So a location of that agent must not hold the name. The two oplog payload
    // namespaces are of that agent name in two components.
    let storage = test.get_blob_storage().await;
    let label = "the_filesystem_snapshots_namespace_gives_each_agent_its_own_location";
    let environment = "0a8cd1b1-5c35-4f0e-9c67-2bb4c0f0f3a1";
    let other_environment = "1b9de2c2-6d46-4a1f-8d78-3cc5d101a4b2";
    let component = "2caef3d3-7e57-4b2a-9e89-4dd6e212b5c3";
    let other_component = "3dbf04e4-8f68-4c3b-8f9a-5ee7f323c6d4";
    let agent = r#"counter("a")"#;
    let dot_segment_agent = r#"counter("a/../b")"#;
    let oplog_payload_of = |component: &str| BlobStorageNamespace::OplogPayload {
        environment_id: EnvironmentId(Uuid::parse_str(environment).unwrap()),
        agent_id: AgentId {
            component_id: ComponentId(Uuid::parse_str(component).unwrap()),
            agent_id: dot_segment_agent.to_string(),
        },
        agent_mode: AgentMode::Durable,
    };
    let namespaces = [
        filesystem_snapshots_of(environment, component, agent),
        filesystem_snapshots_of(environment, component, dot_segment_agent),
        filesystem_snapshots_of(other_environment, component, agent),
        filesystem_snapshots_of(environment, other_component, agent),
        oplog_payload_of(component),
        oplog_payload_of(other_component),
        BlobStorageNamespace::CustomStorage {
            environment_id: EnvironmentId(Uuid::parse_str(environment).unwrap()),
        },
    ];
    let path = Path::new("tree/blob");
    futures::stream::iter(namespaces.iter().enumerate())
        .then(|(index, namespace)| {
            let storage = storage.clone();
            async move {
                storage
                    .put_raw(
                        label,
                        "put-raw",
                        namespace.clone(),
                        path,
                        format!("namespace {index}").as_bytes(),
                    )
                    .await
            }
        })
        .try_collect::<Vec<_>>()
        .await
        .unwrap();

    let read = |namespace: BlobStorageNamespace| {
        let storage = storage.clone();
        async move {
            (
                storage
                    .get_raw(label, "get-raw", namespace.clone(), path)
                    .await
                    .unwrap(),
                sorted_listing(&storage, &namespace, "").await,
            )
        }
    };
    let before_delete = futures::stream::iter(namespaces.clone())
        .then(read)
        .collect::<Vec<_>>()
        .await;
    storage
        .delete(label, "delete", namespaces[0].clone(), path)
        .await
        .unwrap();
    let after_delete = futures::stream::iter(namespaces.clone())
        .then(read)
        .collect::<Vec<_>>()
        .await;

    let own_blob = |index: usize| {
        (
            Some(format!("namespace {index}").into_bytes()),
            listed_blobs(&[("tree/blob", format!("namespace {index}").len())]),
        )
    };
    assert_eq!(
        before_delete,
        (0..namespaces.len()).map(own_blob).collect::<Vec<_>>()
    );
    assert_eq!(
        after_delete,
        std::iter::once((None, Vec::new()))
            .chain((1..namespaces.len()).map(own_blob))
            .collect::<Vec<_>>()
    );
}
