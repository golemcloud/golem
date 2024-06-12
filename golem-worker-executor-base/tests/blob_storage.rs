// Copyright 2024 Golem Cloud
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
use once_cell::sync::Lazy;
use tempfile::{tempdir, TempDir};
use testcontainers::Container;
use testcontainers_modules::minio::MinIO;
use uuid::Uuid;

use golem_worker_executor_base::services::golem_config::S3BlobStorageConfig;
use golem_worker_executor_base::storage::blob::{
    fs, memory, s3, BlobStorage, BlobStorageNamespace,
};

macro_rules! test_blob_storage {
    ( $name:ident, $init:expr, $ns:expr ) => {
        mod $name {
            use assert2::check;
            use bytes::Bytes;
            use golem_worker_executor_base::storage::blob::*;
            use std::path::Path;

            use crate::blob_storage::GetBlobStorage;

            #[tokio::test]
            #[tracing::instrument]
            async fn get_put_get_root() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

                let path = Path::new("test-path");
                let data = Bytes::from("test-data");

                let result1 = storage
                    .get_raw("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();

                storage
                    .put_raw("test-target", "test-op", namespace.clone(), path, &data)
                    .await
                    .unwrap();

                let result2 = storage
                    .get_raw("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();

                check!(result1 == None);
                check!(result2 == Some(data));
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn get_put_get_new_dir() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

                let path = Path::new("non-existing-dir/test-path");
                let data = Bytes::from("test-data");

                let result1 = storage
                    .get_raw("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();

                storage
                    .put_raw("test-target", "test-op", namespace.clone(), path, &data)
                    .await
                    .unwrap();

                let result2 = storage
                    .get_raw("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();

                check!(result1 == None);
                check!(result2 == Some(data));
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn create_delete_exists_dir() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

                let path = Path::new("test-dir");

                let result1 = storage
                    .exists("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();
                storage
                    .create_dir("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();
                let result2 = storage
                    .exists("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();
                storage
                    .delete_dir("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();
                let result3 = storage
                    .exists("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();

                check!(result1 == ExistsResult::DoesNotExist);
                check!(result2 == ExistsResult::Directory);
                check!(result3 == ExistsResult::DoesNotExist);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn create_delete_exists_dir_and_file() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

                let path = Path::new("test-dir");

                let result1 = storage
                    .exists("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();
                storage
                    .create_dir("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();
                storage
                    .put_raw(
                        "test-target",
                        "test-op",
                        namespace.clone(),
                        &path.join("test-file"),
                        &Bytes::from("test-data"),
                    )
                    .await
                    .unwrap();
                let result2 = storage
                    .exists("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();
                let result3 = storage
                    .exists(
                        "test-target",
                        "test-op",
                        namespace.clone(),
                        &path.join("test-file"),
                    )
                    .await
                    .unwrap();
                storage
                    .delete_dir("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();
                let result4 = storage
                    .exists("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();

                check!(result1 == ExistsResult::DoesNotExist);
                check!(result2 == ExistsResult::Directory);
                check!(result3 == ExistsResult::File);
                check!(result4 == ExistsResult::DoesNotExist);
            }

            #[tokio::test]
            #[tracing::instrument]
            async fn list_dir() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

                let path = Path::new("test-dir");
                storage
                    .create_dir("test-target", "test-op", namespace.clone(), path)
                    .await
                    .unwrap();
                storage
                    .put_raw(
                        "test-target",
                        "test-op",
                        namespace.clone(),
                        &path.join("test-file1"),
                        &Bytes::from("test-data1"),
                    )
                    .await
                    .unwrap();
                storage
                    .put_raw(
                        "test-target",
                        "test-op",
                        namespace.clone(),
                        &path.join("test-file2"),
                        &Bytes::from("test-data2"),
                    )
                    .await
                    .unwrap();
                storage
                    .create_dir(
                        "test-target",
                        "test-op",
                        namespace.clone(),
                        &path.join("inner-dir"),
                    )
                    .await
                    .unwrap();
                let mut entries = storage
                    .list_dir("test-target", "test-op", namespace.clone(), path)
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

            #[tokio::test]
            #[tracing::instrument]
            async fn list_dir_root() {
                let test = $init().await;
                let storage = test.get_blob_storage();
                let namespace = $ns();

                storage
                    .put_raw(
                        "test-target",
                        "test-op",
                        namespace.clone(),
                        Path::new("test-file1"),
                        &Bytes::from("test-data1"),
                    )
                    .await
                    .unwrap();
                storage
                    .put_raw(
                        "test-target",
                        "test-op",
                        namespace.clone(),
                        Path::new("test-file2"),
                        &Bytes::from("test-data2"),
                    )
                    .await
                    .unwrap();
                storage
                    .create_dir(
                        "test-target",
                        "test-op",
                        namespace.clone(),
                        Path::new("inner-dir"),
                    )
                    .await
                    .unwrap();
                let mut entries = storage
                    .list_dir("test-target", "test-op", namespace.clone(), Path::new(""))
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
        }
    };
}

pub(crate) trait GetBlobStorage {
    fn get_blob_storage(&self) -> &dyn BlobStorage;
}

struct InMemoryTest {
    storage: memory::InMemoryBlobStorage,
}

impl GetBlobStorage for InMemoryTest {
    fn get_blob_storage(&self) -> &dyn BlobStorage {
        &self.storage
    }
}

struct FsTest {
    _dir: TempDir,
    storage: fs::FileSystemBlobStorage,
}

impl GetBlobStorage for FsTest {
    fn get_blob_storage(&self) -> &dyn BlobStorage {
        &self.storage
    }
}

struct S3Test<'a> {
    _container: Container<'a, MinIO>,
    storage: s3::S3BlobStorage,
}

impl<'a> GetBlobStorage for S3Test<'a> {
    fn get_blob_storage(&self) -> &dyn BlobStorage {
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

// Using a global docker client to avoid the restrictions of the testcontainers library,
// binding the container lifetime to the client.
static DOCKER: Lazy<testcontainers::clients::Cli> =
    Lazy::new(testcontainers::clients::Cli::default);

pub(crate) async fn s3() -> impl GetBlobStorage {
    let minio = MinIO::default();
    let node = DOCKER.run(minio);
    let host_port = node.get_host_port_ipv4(9000);

    let config = S3BlobStorageConfig {
        retries: Default::default(),
        region: "us-east-1".to_string(),
        object_prefix: "".to_string(),
        aws_endpoint_url: Some(format!("http://127.0.0.1:{host_port}")),
        minio: true,
        ..std::default::Default::default()
    };
    create_buckets(host_port, &config).await;
    S3Test {
        _container: node,
        storage: s3::S3BlobStorage::new(config.clone()).await,
    }
}

pub(crate) async fn s3_prefixed() -> impl GetBlobStorage {
    let minio = MinIO::default();
    let node = DOCKER.run(minio);
    let host_port = node.get_host_port_ipv4(9000);

    let config = S3BlobStorageConfig {
        retries: Default::default(),
        region: "us-east-1".to_string(),
        object_prefix: "test-prefix".to_string(),
        aws_endpoint_url: Some(format!("http://127.0.0.1:{host_port}")),
        minio: true,
        ..std::default::Default::default()
    };
    create_buckets(host_port, &config).await;
    S3Test {
        _container: node,
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
