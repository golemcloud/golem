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

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use figment::providers::{Env, Format, Toml};
use figment::Figment;
use http::Uri;
use serde::Deserialize;
use url::Url;

use golem_common::config::{RedisConfig, RetryConfig};

/// The shared global Golem configuration
#[derive(Clone, Debug, Deserialize)]
pub struct GolemConfig {
    pub key_value_storage: KeyValueStorageConfig,
    pub indexed_storage: IndexedStorageConfig,
    pub blob_storage: BlobStorageConfig,
    pub limits: Limits,
    pub retry: RetryConfig,
    pub component_cache: ComponentCacheConfig,
    pub component_service: ComponentServiceConfig,
    pub compiled_component_service: CompiledComponentServiceConfig,
    pub shard_manager_service: ShardManagerServiceConfig,
    pub oplog: OplogConfig,
    pub suspend: SuspendConfig,
    pub active_workers: ActiveWorkersConfig,
    pub scheduler: SchedulerConfig,
    pub public_worker_api: WorkerServiceGrpcConfig,
    pub memory: MemoryConfig,
    pub enable_tracing_console: bool,
    pub enable_json_log: bool,
    pub grpc_address: String,
    pub port: u16,
    pub http_address: String,
    pub http_port: u16,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Limits {
    pub max_active_workers: usize,
    pub concurrency_limit_per_connection: usize,
    pub max_concurrent_streams: u32,
    pub event_broadcast_capacity: usize,
    pub event_history_size: usize,
    pub fuel_to_borrow: i64,
    #[serde(with = "humantime_serde")]
    pub epoch_interval: Duration,
    pub epoch_ticks: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ComponentCacheConfig {
    pub max_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ComponentServiceConfig {
    Grpc(ComponentServiceGrpcConfig),
    Local(ComponentServiceLocalConfig),
}

#[derive(Clone, Debug, Deserialize)]
pub struct ComponentServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
    pub retries: RetryConfig,
    pub max_component_size: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ComponentServiceLocalConfig {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum CompiledComponentServiceConfig {
    Enabled(CompiledComponentServiceEnabledConfig),
    Disabled(CompiledComponentServiceDisabledConfig),
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompiledComponentServiceEnabledConfig {}

#[derive(Clone, Debug, Deserialize)]
pub struct CompiledComponentServiceDisabledConfig {}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ShardManagerServiceConfig {
    Grpc(ShardManagerServiceGrpcConfig),
    SingleShard,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ShardManagerServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub retries: RetryConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorkerServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub access_token: String,
}

impl GolemConfig {
    pub fn new() -> Self {
        Figment::new()
            .merge(Toml::file("config/worker-executor.toml"))
            .merge(Env::prefixed("GOLEM__").split("__"))
            .extract()
            .expect("Failed to parse config")
    }

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

impl ShardManagerServiceGrpcConfig {
    pub fn url(&self) -> Url {
        Url::parse(&format!("http://{}:{}", self.host, self.port))
            .expect("Failed to parse shard manager URL")
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

#[derive(Clone, Debug, Deserialize)]
pub struct SuspendConfig {
    #[serde(with = "humantime_serde")]
    pub suspend_after: Duration,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ActiveWorkersConfig {
    pub drop_when_full: f64,
    #[serde(with = "humantime_serde")]
    pub ttl: Duration,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SchedulerConfig {
    #[serde(with = "humantime_serde")]
    pub refresh_interval: Duration,
}

#[derive(Clone, Debug, Deserialize)]
pub struct OplogConfig {
    pub max_operations_before_commit: u64,
    pub max_payload_size: usize,
    pub indexed_storage_layers: usize,
    pub blob_storage_layers: usize,
    pub entry_count_limit: u64,
    #[serde(with = "humantime_serde")]
    pub archive_interval: Duration,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum KeyValueStorageConfig {
    Redis(RedisConfig),
    InMemory,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum IndexedStorageConfig {
    KVStoreRedis,
    Redis(RedisConfig),
    InMemory,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum BlobStorageConfig {
    S3(S3BlobStorageConfig),
    LocalFileSystem(LocalFileSystemBlobStorageConfig),
    InMemory,
}

#[derive(Clone, Debug, Deserialize)]
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
}

#[derive(Clone, Debug, Deserialize)]
pub struct LocalFileSystemBlobStorageConfig {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MemoryConfig {
    pub system_memory_override: Option<u64>,
    pub worker_memory_ratio: f64,
    pub worker_estimate_coefficient: f64,
}

impl MemoryConfig {
    pub fn total_system_memory(&self) -> u64 {
        let mut sysinfo = sysinfo::System::new();
        sysinfo.refresh_memory();
        sysinfo.total_memory()
    }

    pub fn system_memory(&self) -> u64 {
        self.system_memory_override.unwrap_or_else(|| {
            let mut sysinfo = sysinfo::System::new();
            sysinfo.refresh_memory();
            sysinfo.available_memory()
        })
    }

    pub fn worker_memory(&self) -> u64 {
        (self.system_memory() as f64 * self.worker_memory_ratio) as u64
    }
}

impl Default for GolemConfig {
    fn default() -> Self {
        Self {
            key_value_storage: KeyValueStorageConfig::default(),
            indexed_storage: IndexedStorageConfig::default(),
            blob_storage: BlobStorageConfig::default(),
            limits: Limits::default(),
            retry: RetryConfig::default(),
            component_cache: ComponentCacheConfig::default(),
            component_service: ComponentServiceConfig::default(),
            compiled_component_service: CompiledComponentServiceConfig::default(),
            shard_manager_service: ShardManagerServiceConfig::default(),
            oplog: OplogConfig::default(),
            suspend: SuspendConfig::default(),
            scheduler: SchedulerConfig::default(),
            active_workers: ActiveWorkersConfig::default(),
            public_worker_api: WorkerServiceGrpcConfig::default(),
            memory: MemoryConfig::default(),
            enable_tracing_console: false,
            enable_json_log: false,
            grpc_address: "0.0.0.0".to_string(),
            port: 9000,
            http_address: "0.0.0.0".to_string(),
            http_port: 8080,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_active_workers: 1024,
            concurrency_limit_per_connection: 1024,
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
            max_capacity: 128,
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
            access_token: "access_token".to_string(),
            retries: RetryConfig::default(),
            max_component_size: 50 * 1024 * 1024,
        }
    }
}

impl Default for CompiledComponentServiceConfig {
    fn default() -> Self {
        Self::Enabled(CompiledComponentServiceEnabledConfig {})
    }
}

impl Default for S3BlobStorageConfig {
    fn default() -> Self {
        Self {
            retries: RetryConfig::default(),
            region: "us-east-1".to_string(),
            compilation_cache_bucket: "golem-compiled-components".to_string(),
            custom_data_bucket: "custom-data".to_string(),
            oplog_payload_bucket: "oplog-payload".to_string(),
            object_prefix: "".to_string(),
            aws_endpoint_url: None,
            compressed_oplog_buckets: vec!["oplog-archive-1".to_string()],
            use_minio_credentials: false,
        }
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
            port: 9020,
            retries: RetryConfig::default(),
        }
    }
}

impl Default for OplogConfig {
    fn default() -> Self {
        Self {
            max_operations_before_commit: 128,
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
            port: 9090,
            access_token: "access_token".to_string(),
        }
    }
}

impl Default for KeyValueStorageConfig {
    fn default() -> Self {
        Self::Redis(RedisConfig::default())
    }
}

impl Default for IndexedStorageConfig {
    fn default() -> Self {
        Self::KVStoreRedis
    }
}

impl Default for BlobStorageConfig {
    fn default() -> Self {
        Self::S3(S3BlobStorageConfig::default())
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            system_memory_override: None,
            worker_memory_ratio: 0.8,
            worker_estimate_coefficient: 1.1,
        }
    }
}
