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

use golem_common::config::DbSqliteConfig;
use golem_common::model::RetryConfig;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerExecutorClientCacheConfig {
    pub max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

impl Default for WorkerExecutorClientCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 1000,
            time_to_idle: Duration::from_secs(60 * 60 * 4),
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum BlobStorageConfig {
    S3(S3BlobStorageConfig),
    LocalFileSystem(LocalFileSystemBlobStorageConfig),
    KVStoreSqlite,
    Sqlite(DbSqliteConfig),
    InMemory,
}

impl Default for BlobStorageConfig {
    fn default() -> Self {
        Self::default_local_file_system()
    }
}

impl BlobStorageConfig {
    pub fn default_s3() -> Self {
        Self::S3(S3BlobStorageConfig::default())
    }

    pub fn default_local_file_system() -> Self {
        Self::LocalFileSystem(LocalFileSystemBlobStorageConfig::default())
    }

    pub fn default_in_memory() -> Self {
        Self::InMemory
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct S3BlobStorageConfig {
    pub retries: RetryConfig,
    pub region: String,
    pub object_prefix: String,
    pub aws_endpoint_url: Option<String>,
    pub compilation_cache_bucket: String,
    pub custom_data_bucket: String,
    pub oplog_payload_bucket: String,
    pub compressed_oplog_buckets: Vec<String>,
    pub use_minio_credentials: bool,
    pub initial_component_files_bucket: String,
}

impl Default for S3BlobStorageConfig {
    fn default() -> Self {
        Self {
            retries: RetryConfig::max_attempts_3(),
            region: "us-east-1".to_string(),
            compilation_cache_bucket: "golem-compiled-components".to_string(),
            custom_data_bucket: "custom-data".to_string(),
            oplog_payload_bucket: "oplog-payload".to_string(),
            object_prefix: "".to_string(),
            aws_endpoint_url: None,
            compressed_oplog_buckets: vec!["oplog-archive-1".to_string()],
            use_minio_credentials: false,
            initial_component_files_bucket: "golem-initial-component-files".to_string(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalFileSystemBlobStorageConfig {
    pub root: PathBuf,
}

impl Default for LocalFileSystemBlobStorageConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("../data/blob_storage"),
        }
    }
}
