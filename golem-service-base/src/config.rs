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

use golem_common::config::DbSqliteConfig;
use golem_common::config::{ConfigLoader, ConfigLoaderConfig};
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
    KVStoreSqlite(KVStoreSqliteBlobStorageConfig),
    Sqlite(DbSqliteConfig),
    InMemory(InMemoryBlobStorageConfig),
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
        Self::InMemory(InMemoryBlobStorageConfig {})
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
    pub components_bucket: String,
    pub plugin_wasm_files_bucket: String,
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
            components_bucket: "component-store".to_string(),
            plugin_wasm_files_bucket: "golem-plugin-wasm-files".to_string(),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KVStoreSqliteBlobStorageConfig {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InMemoryBlobStorageConfig {}

pub struct MergedConfigLoader<T> {
    config_file_name: PathBuf,
    config: figment::Result<T>,
}

impl<T: ConfigLoaderConfig> MergedConfigLoader<T> {
    pub fn new(name: &str, config_loader: ConfigLoader<T>) -> MergedConfigLoader<T> {
        MergedConfigLoader {
            config_file_name: config_loader.config_file_name.clone(),
            config: Ok(()),
        }
        .add(name, config_loader, |_, config| config)
    }

    pub fn add<U: ConfigLoaderConfig, V>(
        self,
        name: &str,
        config_loader: ConfigLoader<U>,
        merge: fn(T, U) -> V,
    ) -> MergedConfigLoader<V> {
        if self.config_file_name != config_loader.config_file_name {
            panic!(
                "config_file_name mismatch while loading for '{}' config: {:?} <-> {:?}",
                name, self.config_file_name, config_loader.config_file_name,
            );
        }

        let config = match self.config {
            Ok(base_config) => match config_loader.load() {
                Ok(config) => Ok(merge(base_config, config)),
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };

        MergedConfigLoader {
            config_file_name: self.config_file_name,
            config,
        }
    }
}

impl<T> MergedConfigLoader<T> {
    pub fn finish(self) -> figment::Result<T> {
        self.config
    }
}

pub struct MergedConfigLoaderOrDumper<T> {
    config_file_name: PathBuf,
    config: Option<T>,
    dummy: bool,
}

impl<T: ConfigLoaderConfig> MergedConfigLoaderOrDumper<T> {
    pub fn new(name: &str, config_loader: ConfigLoader<T>) -> MergedConfigLoaderOrDumper<T> {
        MergedConfigLoaderOrDumper {
            config_file_name: config_loader.config_file_name.clone(),
            config: Some(()),
            dummy: true,
        }
        .add(name, config_loader, |_, config| config)
    }

    pub fn add<U: ConfigLoaderConfig, V>(
        self,
        name: &str,
        config_loader: ConfigLoader<U>,
        merge: fn(T, U) -> V,
    ) -> MergedConfigLoaderOrDumper<V> {
        if self.config_file_name != config_loader.config_file_name {
            panic!(
                "config_file_name mismatch while loading (or dumping) for '{}' config: {:?} <-> {:?}",
                name, self.config_file_name, config_loader.config_file_name,
            );
        }

        let config = match self.config {
            Some(base_config) => match config_loader.load_or_dump_config() {
                Some(config) => Some(merge(base_config, config)),
                None if self.dummy => None,
                None => {
                    panic!("illegal state while dumping, got no config for '{}'", name,);
                }
            },
            None => {
                match config_loader.load_or_dump_config() {
                    Some(_) => {
                        panic!("illegal state while loading, got config for '{}', while expected dumping", name);
                    }
                    None => None,
                }
            }
        };

        MergedConfigLoaderOrDumper {
            config_file_name: self.config_file_name,
            config,
            dummy: false,
        }
    }
}

impl<T> MergedConfigLoaderOrDumper<T> {
    pub fn finish(self) -> Option<T> {
        self.config
    }
}
