// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use aws_config::meta::region::RegionProviderChain;
use aws_config::BehaviorVersion;
use aws_sdk_s3::config::Credentials;
use aws_sdk_s3::Client;
use golem_common::model::{AccountId, ComponentId};
use golem_service_base::config::S3BlobStorageConfig;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::{fs, memory, s3, BlobStorage, BlobStorageNamespace};
use golem_service_base::storage::sqlite::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use tempfile::{tempdir, TempDir};
use testcontainers::runners::AsyncRunner;
use testcontainers::ContainerAsync;
use testcontainers_modules::minio::MinIO;
use uuid::Uuid;

macro_rules! test_blob_storage {
    ( $name:ident, $init:expr, $ns:expr ) => {
        mod $name {
            use test_r::test;

            use assert2::check;
            use bytes::{BufMut, Bytes, BytesMut};
            use futures::TryStreamExt;
            use golem_service_base::storage::blob::*;
            use std::path::Path;

            use crate::blob_storage::GetBlobStorage;

            #[test]
            #[tracing::instrument]
            async fn get_put_get_root() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

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
            async fn get_put_get_new_dir() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

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
            async fn get_put_get_new_dir_streaming() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

                let path = Path::new("non-existing-dir/test-path");
                let mut data = BytesMut::new();
                for n in 1..(10 * 1024 * 1024) {
                    data.put_u8((n % 100) as u8);
                }
                let data = data.freeze();

                let result1 = storage
                    .get_stream("get_put_get_new_dir", "get-raw", namespace.clone(), path)
                    .await
                    .unwrap();

                storage
                    .put_stream(
                        "get_put_get_new_dir",
                        "put-raw",
                        namespace.clone(),
                        path,
                        &data,
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
            async fn create_delete_exists_dir() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

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
            async fn create_delete_exists_dir_and_file() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

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
            async fn list_dir() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

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
            async fn delete_many() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

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
            async fn list_dir_root() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

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
            async fn list_dir_root_only_subdirs() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

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
            async fn list_dir_same_prefix() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

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
        }
    };
}

pub(crate) trait GetBlobStorage {
    fn get_blob_storage(&self) -> &(dyn BlobStorage + Send + Sync);
}

struct InMemoryTest {
    storage: memory::InMemoryBlobStorage,
}

impl GetBlobStorage for InMemoryTest {
    fn get_blob_storage(&self) -> &(dyn BlobStorage + Send + Sync) {
        &self.storage
    }
}

struct FsTest {
    _dir: TempDir,
    storage: fs::FileSystemBlobStorage,
}

impl GetBlobStorage for FsTest {
    fn get_blob_storage(&self) -> &(dyn BlobStorage + Send + Sync) {
        &self.storage
    }
}

struct S3Test {
    _container: ContainerAsync<MinIO>,
    storage: s3::S3BlobStorage,
}

impl GetBlobStorage for S3Test {
    fn get_blob_storage(&self) -> &(dyn BlobStorage + Send + Sync) {
        &self.storage
    }
}

struct SqliteTest {
    storage: SqliteBlobStorage,
}

impl GetBlobStorage for SqliteTest {
    fn get_blob_storage(&self) -> &(dyn BlobStorage + Send + Sync) {
        &self.storage
    }
}

pub(crate) async fn in_memory() -> impl GetBlobStorage {
    InMemoryTest {
        storage: memory::InMemoryBlobStorage::new(),
    }
}

pub(crate) async fn fs() -> impl GetBlobStorage {
    let dir = tempdir().unwrap();
    let path = dir.path().to_path_buf();
    FsTest {
        _dir: dir,
        storage: fs::FileSystemBlobStorage::new(&path).await.unwrap(),
    }
}

pub(crate) async fn s3() -> impl GetBlobStorage {
    let container = MinIO::default()
        .start()
        .await
        .expect("Failed to start MinIO");
    let host_port = container
        .get_host_port_ipv4(9000)
        .await
        .expect("Failed to get host port");

    let config = S3BlobStorageConfig {
        retries: Default::default(),
        region: "us-east-1".to_string(),
        object_prefix: "".to_string(),
        aws_endpoint_url: Some(format!("http://127.0.0.1:{host_port}")),
        use_minio_credentials: true,
        ..std::default::Default::default()
    };
    create_buckets(host_port, &config).await;
    S3Test {
        _container: container,
        storage: s3::S3BlobStorage::new(config.clone()).await,
    }
}

pub(crate) async fn s3_prefixed() -> impl GetBlobStorage {
    let container = MinIO::default()
        .start()
        .await
        .expect("Failed to start MinIO");
    let host_port = container
        .get_host_port_ipv4(9000)
        .await
        .expect("Failed to get host port");

    let config = S3BlobStorageConfig {
        retries: Default::default(),
        region: "us-east-1".to_string(),
        object_prefix: "test-prefix".to_string(),
        aws_endpoint_url: Some(format!("http://127.0.0.1:{host_port}")),
        use_minio_credentials: true,
        ..std::default::Default::default()
    };
    create_buckets(host_port, &config).await;
    S3Test {
        _container: container,
        storage: s3::S3BlobStorage::new(config.clone()).await,
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

pub(crate) fn compilation_cache() -> BlobStorageNamespace {
    BlobStorageNamespace::CompilationCache
}

pub(crate) fn compressed_oplog() -> BlobStorageNamespace {
    BlobStorageNamespace::CompressedOplog {
        account_id: AccountId {
            value: "test-account".to_string(),
        },
        component_id: ComponentId(Uuid::new_v4()),
        level: 0,
    }
}

pub(crate) async fn sqlite() -> impl GetBlobStorage {
    let sqlx_pool_sqlite = SqlitePoolOptions::new()
        .max_connections(10)
        .connect("sqlite::memory:")
        .await
        .expect("Cannot create db options");

    let pool = SqlitePool::new(sqlx_pool_sqlite)
        .await
        .expect("Cannot connect to sqlite db");

    let sbs = SqliteBlobStorage::new(pool).await.unwrap();

    SqliteTest { storage: sbs }
}

test_blob_storage!(
    in_memory_cc,
    crate::blob_storage::in_memory,
    crate::blob_storage::compilation_cache
);
test_blob_storage!(
    filesystem_cc,
    crate::blob_storage::fs,
    crate::blob_storage::compilation_cache
);
test_blob_storage!(
    s3_no_prefix_cc,
    crate::blob_storage::s3,
    crate::blob_storage::compilation_cache
);
test_blob_storage!(
    s3_prefixed_cc,
    crate::blob_storage::s3_prefixed,
    crate::blob_storage::compilation_cache
);
test_blob_storage!(
    sqlite_cc,
    crate::blob_storage::sqlite,
    crate::blob_storage::compilation_cache
);

test_blob_storage!(
    in_memory_co,
    crate::blob_storage::in_memory,
    crate::blob_storage::compressed_oplog
);
test_blob_storage!(
    filesystem_co,
    crate::blob_storage::fs,
    crate::blob_storage::compressed_oplog
);
test_blob_storage!(
    s3_no_prefix_co,
    crate::blob_storage::s3,
    crate::blob_storage::compressed_oplog
);
test_blob_storage!(
    s3_prefixed_co,
    crate::blob_storage::s3_prefixed,
    crate::blob_storage::compressed_oplog
);
test_blob_storage!(
    sqlite_co,
    crate::blob_storage::sqlite,
    crate::blob_storage::compressed_oplog
);
