// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use assert2::check;
use async_trait::async_trait;
use aws_config::meta::region::RegionProviderChain;
use aws_config::BehaviorVersion;
use aws_sdk_s3::config::Credentials;
use aws_sdk_s3::Client;
use bytes::{BufMut, Bytes, BytesMut};
use futures::stream::BoxStream;
use futures::TryStreamExt;
use golem_common::model::{AccountId, ComponentId};
use golem_common::widen_infallible;
use golem_service_base::config::S3BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::*;
use golem_service_base::storage::blob::{fs, memory, s3, BlobStorage, BlobStorageNamespace};
use sqlx::sqlite::SqlitePoolOptions;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Duration;
use tempfile::{tempdir, TempDir};
use test_r::{define_matrix_dimension, test, test_dep};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::minio::MinIO;
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

#[test_dep(tagged_as = "in_memory")]
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
        let path = self.dir.path().join(format!("test-{}", counter));
        Arc::new(fs::FileSystemBlobStorage::new(&path).await.unwrap())
    }
}

#[test_dep(tagged_as = "fs")]
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
        let container = tryhard::retry_fn(|| MinIO::default().start())
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
            use_minio_credentials: true,
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
    for bucket in &config.compressed_oplog_buckets {
        client.create_bucket().bucket(bucket).send().await.unwrap();
    }
}

struct S3BlobStorageWithContainer {
    storage: s3::S3BlobStorage,
    _container: ContainerAsync<MinIO>,
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
    ) -> Result<Option<Bytes>, String> {
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
    ) -> Result<Option<BoxStream<'static, Result<Bytes, String>>>, String> {
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
    ) -> Result<Option<Bytes>, String> {
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
    ) -> Result<Option<BlobMetadata>, String> {
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
    ) -> Result<(), String> {
        self.storage
            .put_raw(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = Result<Bytes, String>, Error = String>,
    ) -> Result<(), String> {
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
    ) -> Result<(), String> {
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
    ) -> Result<(), String> {
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
    ) -> Result<(), String> {
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
    ) -> Result<Vec<PathBuf>, String> {
        self.storage
            .list_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> Result<bool, String> {
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
    ) -> Result<ExistsResult, String> {
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
    ) -> Result<(), String> {
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
    ) -> Result<(), String> {
        self.storage
            .r#move(target_label, op_label, namespace, from, to)
            .await
    }
}

#[test_dep(tagged_as = "s3")]
async fn s3() -> Arc<dyn GetBlobStorage + Send + Sync> {
    Arc::new(S3Test { prefixed: None })
}

#[test_dep(tagged_as = "s3_prefixed")]
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

#[test_dep(tagged_as = "sqlite")]
async fn sqlite() -> Arc<dyn GetBlobStorage + Send + Sync> {
    Arc::new(SqliteTest)
}

#[test_dep(tagged_as = "cc")]
fn compilation_cache() -> BlobStorageNamespace {
    BlobStorageNamespace::CompilationCache
}

#[test_dep(tagged_as = "co")]
fn compressed_oplog() -> BlobStorageNamespace {
    BlobStorageNamespace::CompressedOplog {
        account_id: AccountId {
            value: "test-account".to_string(),
        },
        component_id: ComponentId(Uuid::new_v4()),
        level: 0,
    }
}

define_matrix_dimension!(storage: Arc<dyn GetBlobStorage + Send + Sync> -> "in_memory", "fs", "s3", "s3_prefixed", "sqlite");
define_matrix_dimension!(ns: BlobStorageNamespace -> "cc", "co");

#[test]
#[tracing::instrument]
async fn get_put_get_root(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let path = Path::new("test-path");
    let data = Bytes::from("test-data");

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

    check!(result1 == None);
    check!(result2 == Some(data));
}

#[test]
#[tracing::instrument]
async fn get_put_get_new_dir(
    #[dimension(storage)] test: &Arc<dyn GetBlobStorage + Send + Sync>,
    #[dimension(ns)] namespace: &BlobStorageNamespace,
) {
    let storage = test.get_blob_storage().await;

    let path = Path::new("non-existing-dir/test-path");
    let data = Bytes::from("test-data");

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

    check!(result1 == None);
    check!(result2 == Some(data));
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
    let data = data.freeze();

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

    check!(result1.is_none());
    check!(result2 == data.to_vec());
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

    check!(result1 == ExistsResult::DoesNotExist);
    check!(result2 == ExistsResult::Directory);
    check!(result3 == ExistsResult::DoesNotExist);
    check!(delete_result1 == true);
    check!(delete_result2 == false);
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

    check!(result1 == ExistsResult::DoesNotExist);
    check!(result2 == ExistsResult::Directory);
    check!(result3 == ExistsResult::File);
    check!(result4 == ExistsResult::DoesNotExist);
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

    check!(
        entries
            == vec![
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

    check!(
        entries
            == vec![
                Path::new("test-dir/inner-dir").to_path_buf(),
                Path::new("test-dir/test-file2").to_path_buf(),
            ]
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

    check!(
        entries
            == vec![
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

    check!(
        entries
            == vec![
                Path::new("inner-dir1").to_path_buf(),
                Path::new("inner-dir2").to_path_buf(),
            ]
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

    check!(
        entries
            == vec![
                Path::new("test-dir/inner-dir").to_path_buf(),
                Path::new("test-dir/test-file1").to_path_buf(),
                Path::new("test-dir/test-file2").to_path_buf(),
            ]
    );
}
