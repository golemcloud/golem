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

use golem_common::SafeDisplay;
use golem_common::config::DbSqliteConfig;
use golem_common::config::{ConfigLoader, ConfigLoaderConfig};
use golem_common::model::RetryConfig;
use serde::{Deserialize, Serialize};
use std::{fmt::Write, path::PathBuf, time::Duration};

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

impl SafeDisplay for BlobStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            BlobStorageConfig::S3(inner) => {
                let _ = writeln!(&mut result, "S3:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            BlobStorageConfig::LocalFileSystem(inner) => {
                let _ = writeln!(&mut result, "local file system:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            BlobStorageConfig::KVStoreSqlite(inner) => {
                let _ = writeln!(&mut result, "sqlite kv-store:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            BlobStorageConfig::Sqlite(inner) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            BlobStorageConfig::InMemory(inner) => {
                let _ = writeln!(&mut result, "in-memory:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
        }

        result
    }
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
pub struct S3BlobStorageCredentialsConfig {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub provider_name: String,
}

impl S3BlobStorageCredentialsConfig {
    pub fn new(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        provider_name: impl Into<String>,
    ) -> Self {
        Self {
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            provider_name: provider_name.into(),
        }
    }
}

impl SafeDisplay for S3BlobStorageCredentialsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "access key id: ****");
        let _ = writeln!(&mut result, "secret access key: ****");
        let _ = writeln!(&mut result, "provider name: {}", self.provider_name);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct S3BlobStorageConfig {
    pub retries: RetryConfig,
    pub region: String,
    pub object_prefix: String,
    pub aws_endpoint_url: Option<String>,
    pub aws_credentials: Option<S3BlobStorageCredentialsConfig>,
    pub aws_path_style: Option<bool>,
    pub compilation_cache_bucket: String,
    pub custom_data_bucket: String,
    pub oplog_payload_bucket: String,
    pub compressed_oplog_buckets: Vec<String>,
    pub initial_component_files_bucket: String,
    pub components_bucket: String,
}

impl SafeDisplay for S3BlobStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());
        let _ = writeln!(&mut result, "region: {}", self.region);
        let _ = writeln!(&mut result, "object_prefix: {}", self.object_prefix);
        if let Some(endpoint_url) = &self.aws_endpoint_url {
            let _ = writeln!(&mut result, "aws_endpoint_url: {endpoint_url}");
        }
        if let Some(aws_credentials) = &self.aws_credentials {
            let _ = writeln!(&mut result, "aws_credentials:");
            let _ = writeln!(&mut result, "{}", aws_credentials.to_safe_string_indented());
        }
        if let Some(path_style) = &self.aws_path_style {
            let _ = writeln!(&mut result, "aws_path_style: {path_style}");
        }
        let _ = writeln!(
            &mut result,
            "compilation cache bucket: {}",
            self.compilation_cache_bucket
        );
        let _ = writeln!(
            &mut result,
            "custom data bucket: {}",
            self.custom_data_bucket
        );
        let _ = writeln!(
            &mut result,
            "oplog payload bucket: {}",
            self.oplog_payload_bucket
        );
        let _ = writeln!(
            &mut result,
            "compressed oplog buckets: {:?}",
            self.compressed_oplog_buckets
        );
        let _ = writeln!(
            &mut result,
            "initial component files bucket: {}",
            self.initial_component_files_bucket
        );
        let _ = writeln!(&mut result, "components bucket: {}", self.components_bucket);

        result
    }
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
            aws_credentials: None,
            aws_path_style: None,
            compressed_oplog_buckets: vec!["oplog-archive-1".to_string()],
            initial_component_files_bucket: "golem-initial-component-files".to_string(),
            components_bucket: "component-store".to_string(),
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

impl SafeDisplay for LocalFileSystemBlobStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "root: {:?}", self.root);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KVStoreSqliteBlobStorageConfig {}

impl SafeDisplay for KVStoreSqliteBlobStorageConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InMemoryBlobStorageConfig {}

impl SafeDisplay for InMemoryBlobStorageConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

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
                    panic!("illegal state while dumping, got no config for '{name}'",);
                }
            },
            None => match config_loader.load_or_dump_config() {
                Some(_) => {
                    panic!(
                        "illegal state while loading, got config for '{name}', while expected dumping"
                    );
                }
                None => None,
            },
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
