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

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use figment::providers::{Format, Toml};
use figment::Figment;
use golem_service_base::config::BlobStorageConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use url::Url;

use golem_common::config::{
    ConfigExample, ConfigLoader, DbSqliteConfig, HasConfigExamples, RedisConfig,
};
use golem_common::model::RetryConfig;
use golem_common::tracing::TracingConfig;

/// The shared global Golem configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GolemConfig {
    pub tracing: TracingConfig,
    pub tracing_file_name_with_port: bool,
    pub key_value_storage: KeyValueStorageConfig,
    pub indexed_storage: IndexedStorageConfig,
    pub blob_storage: BlobStorageConfig,
    pub limits: Limits,
    pub retry: RetryConfig,
    pub component_cache: ComponentCacheConfig,
    pub component_service: ComponentServiceConfig,
    pub compiled_component_service: CompiledComponentServiceConfig,
    pub shard_manager_service: ShardManagerServiceConfig,
    pub plugin_service: PluginServiceConfig,
    pub oplog: OplogConfig,
    pub suspend: SuspendConfig,
    pub active_workers: ActiveWorkersConfig,
    pub scheduler: SchedulerConfig,
    pub public_worker_api: WorkerServiceGrpcConfig,
    pub memory: MemoryConfig,
    pub grpc_address: String,
    pub port: u16,
    pub http_address: String,
    pub http_port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limits {
    pub max_active_workers: usize,
    pub invocation_result_broadcast_capacity: usize,
    pub max_concurrent_streams: u32,
    pub event_broadcast_capacity: usize,
    pub event_history_size: usize,
    pub fuel_to_borrow: i64,
    #[serde(with = "humantime_serde")]
    pub epoch_interval: Duration,
    pub epoch_ticks: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentCacheConfig {
    pub max_capacity: usize,
    pub max_metadata_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ComponentServiceConfig {
    Grpc(ComponentServiceGrpcConfig),
    Local(ComponentServiceLocalConfig),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
    pub retries: RetryConfig,
    pub max_component_size: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentServiceLocalConfig {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum PluginServiceConfig {
    Grpc(PluginServiceGrpcConfig),
    Local(PluginServiceLocalConfig),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
    pub retries: RetryConfig,
    pub plugin_cache_size: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginServiceLocalConfig {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum CompiledComponentServiceConfig {
    Enabled(CompiledComponentServiceEnabledConfig),
    Disabled(CompiledComponentServiceDisabledConfig),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompiledComponentServiceEnabledConfig {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompiledComponentServiceDisabledConfig {}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ShardManagerServiceConfig {
    Grpc(ShardManagerServiceGrpcConfig),
    SingleShard,
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardManagerServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub retries: RetryConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
}

impl GolemConfig {
    pub fn from_file(path: &str) -> Self {
        Figment::new()
            .merge(Toml::file(path))
            .extract()
            .expect("Failed to parse config")
    }

    pub fn grpc_addr(&self) -> anyhow::Result<SocketAddr> {
        format!("{}:{}", self.grpc_address, self.port)
            .parse::<SocketAddr>()
            .context("grpc_address configuration")
    }

    pub fn http_addr(&self) -> anyhow::Result<SocketAddrV4> {
        Ok(SocketAddrV4::new(
            self.http_address
                .parse::<Ipv4Addr>()
                .context("http_address configuration")?,
            self.http_port,
        ))
    }

    pub fn add_port_to_tracing_file_name_if_enabled(&mut self) {
        if self.tracing_file_name_with_port {
            if let Some(file_name) = &self.tracing.file_name {
                let elems: Vec<&str> = file_name.split('.').collect();
                self.tracing.file_name = {
                    if elems.len() == 2 {
                        Some(format!("{}.{}.{}", elems[0], self.port, elems[1]))
                    } else {
                        Some(format!("{}.{}", file_name, self.port))
                    }
                }
            }
        }
    }
}

impl ComponentServiceGrpcConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse component service URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build component service URI")
    }
}

impl PluginServiceGrpcConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse plugin service URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build plugin service URI")
    }
}

impl ShardManagerServiceGrpcConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse shard manager URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build shard manager URI")
    }
}

impl WorkerServiceGrpcConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse worker service URL")
    }

    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build worker service URI")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuspendConfig {
    #[serde(with = "humantime_serde")]
    pub suspend_after: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActiveWorkersConfig {
    pub drop_when_full: f64,
    #[serde(with = "humantime_serde")]
    pub ttl: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulerConfig {
    #[serde(with = "humantime_serde")]
    pub refresh_interval: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OplogConfig {
    pub max_operations_before_commit: u64,
    pub max_operations_before_commit_ephemeral: u64,
    pub max_payload_size: usize,
    pub indexed_storage_layers: usize,
    pub blob_storage_layers: usize,
    pub entry_count_limit: u64,
    #[serde(with = "humantime_serde")]
    pub archive_interval: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum KeyValueStorageConfig {
    Redis(RedisConfig),
    Sqlite(DbSqliteConfig),
    InMemory,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum IndexedStorageConfig {
    KVStoreRedis,
    Redis(RedisConfig),
    KVStoreSqlite,
    Sqlite(DbSqliteConfig),
    InMemory,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub system_memory_override: Option<u64>,
    pub worker_memory_ratio: f64,
    pub worker_estimate_coefficient: f64,
    #[serde(with = "humantime_serde")]
    pub acquire_retry_delay: Duration,
    pub oom_retry_config: RetryConfig,
}

impl MemoryConfig {
    pub fn total_system_memory(&self) -> u64 {
        self.system_memory_override.unwrap_or_else(|| {
            let mut sysinfo = sysinfo::System::new();
            sysinfo.refresh_memory();
            sysinfo.total_memory()
        })
    }

    pub fn system_memory(&self) -> u64 {
        let mut sysinfo = sysinfo::System::new();
        sysinfo.refresh_memory();
        sysinfo.available_memory()
    }

    pub fn worker_memory(&self) -> usize {
        (self.total_system_memory() as f64 * self.worker_memory_ratio) as usize
    }
}

impl Default for GolemConfig {
    fn default() -> Self {
        Self {
            tracing: TracingConfig::local_dev("worker-executor"),
            tracing_file_name_with_port: true,
            key_value_storage: KeyValueStorageConfig::default(),
            indexed_storage: IndexedStorageConfig::default(),
            blob_storage: BlobStorageConfig::default(),
            limits: Limits::default(),
            retry: RetryConfig::max_attempts_3(),
            component_cache: ComponentCacheConfig::default(),
            component_service: ComponentServiceConfig::default(),
            compiled_component_service: CompiledComponentServiceConfig::default(),
            shard_manager_service: ShardManagerServiceConfig::default(),
            plugin_service: PluginServiceConfig::default(),
            oplog: OplogConfig::default(),
            suspend: SuspendConfig::default(),
            scheduler: SchedulerConfig::default(),
            active_workers: ActiveWorkersConfig::default(),
            public_worker_api: WorkerServiceGrpcConfig::default(),
            memory: MemoryConfig::default(),
            grpc_address: "0.0.0.0".to_string(),
            port: 9000,
            http_address: "0.0.0.0".to_string(),
            http_port: 8082,
        }
    }
}

impl HasConfigExamples<GolemConfig> for GolemConfig {
    fn examples() -> Vec<ConfigExample<GolemConfig>> {
        vec![
            (
                "with redis indexed_storage, s3 blob storage, single shard manager service",
                Self {
                    key_value_storage: KeyValueStorageConfig::InMemory,
                    indexed_storage: IndexedStorageConfig::Redis(RedisConfig::default()),
                    blob_storage: BlobStorageConfig::default_s3(),
                    shard_manager_service: ShardManagerServiceConfig::SingleShard,
                    ..Self::default()
                },
            ),
            (
                "with in-memory key value storage, indexed storage and blob storage",
                Self {
                    key_value_storage: KeyValueStorageConfig::InMemory,
                    indexed_storage: IndexedStorageConfig::InMemory,
                    blob_storage: BlobStorageConfig::default_in_memory(),
                    ..Self::default()
                },
            ),
        ]
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_active_workers: 1024,
            invocation_result_broadcast_capacity: 100000,
            max_concurrent_streams: 1024,
            event_broadcast_capacity: 16,
            event_history_size: 128,
            fuel_to_borrow: 10000,
            epoch_interval: Duration::from_millis(10),
            epoch_ticks: 1,
        }
    }
}

impl Default for ComponentCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 32,
            max_metadata_capacity: 16384,
            time_to_idle: Duration::from_secs(12 * 60 * 60),
        }
    }
}

impl Default for ComponentServiceConfig {
    fn default() -> Self {
        Self::Grpc(ComponentServiceGrpcConfig::default())
    }
}

impl Default for ComponentServiceGrpcConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9090,
            access_token: "2a354594-7a63-4091-a46b-cc58d379f677".to_string(),
            retries: RetryConfig::max_attempts_3(),
            max_component_size: 50 * 1024 * 1024,
        }
    }
}

impl Default for PluginServiceConfig {
    fn default() -> Self {
        Self::Grpc(PluginServiceGrpcConfig::default())
    }
}

impl Default for PluginServiceGrpcConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9090,
            access_token: "2a354594-7a63-4091-a46b-cc58d379f677".to_string(),
            retries: RetryConfig::max_attempts_3(),
            plugin_cache_size: 1024,
        }
    }
}

impl Default for CompiledComponentServiceConfig {
    fn default() -> Self {
        Self::enabled()
    }
}

impl CompiledComponentServiceConfig {
    pub fn enabled() -> Self {
        Self::Enabled(CompiledComponentServiceEnabledConfig {})
    }

    pub fn disabled() -> Self {
        Self::Disabled(CompiledComponentServiceDisabledConfig {})
    }
}

impl Default for ShardManagerServiceConfig {
    fn default() -> Self {
        Self::Grpc(ShardManagerServiceGrpcConfig::default())
    }
}

impl Default for ShardManagerServiceGrpcConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9002,
            retries: RetryConfig::default(),
        }
    }
}

impl Default for OplogConfig {
    fn default() -> Self {
        Self {
            max_operations_before_commit: 128,
            max_operations_before_commit_ephemeral: 512,
            max_payload_size: 64 * 1024,
            indexed_storage_layers: 2,
            blob_storage_layers: 1,
            entry_count_limit: 1024,
            archive_interval: Duration::from_secs(60 * 60 * 24), // 24 hours
        }
    }
}

impl Default for SuspendConfig {
    fn default() -> Self {
        Self {
            suspend_after: Duration::from_secs(10),
        }
    }
}

impl Default for ActiveWorkersConfig {
    fn default() -> Self {
        Self {
            drop_when_full: 0.25,
            ttl: Duration::from_secs(60 * 60 * 8),
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            refresh_interval: Duration::from_secs(2),
        }
    }
}

impl Default for WorkerServiceGrpcConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9007,
            access_token: "2a354594-7a63-4091-a46b-cc58d379f677".to_string(),
        }
    }
}

impl Default for KeyValueStorageConfig {
    fn default() -> Self {
        Self::default_redis()
    }
}

impl KeyValueStorageConfig {
    pub fn default_redis() -> Self {
        Self::Redis(RedisConfig::default())
    }
}

impl Default for IndexedStorageConfig {
    fn default() -> Self {
        Self::KVStoreRedis
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            system_memory_override: None,
            worker_memory_ratio: 0.8,
            worker_estimate_coefficient: 1.1,
            acquire_retry_delay: Duration::from_millis(500),
            oom_retry_config: RetryConfig {
                max_attempts: u32::MAX,
                min_delay: Duration::from_millis(100),
                max_delay: Duration::from_secs(5),
                multiplier: 2.0,
                max_jitter_factor: None, // TODO: should we add jitter here?
            },
        }
    }
}

pub fn make_config_loader() -> ConfigLoader<GolemConfig> {
    ConfigLoader::new_with_examples(Path::new("config/worker-executor.toml"))
}
