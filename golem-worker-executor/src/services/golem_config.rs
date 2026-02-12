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

use anyhow::Context;
use figment::providers::{Format, Toml};
use figment::Figment;
use golem_common::config::{
    ConfigExample, ConfigLoader, DbSqliteConfig, HasConfigExamples, RedisConfig,
};
use golem_common::model::base64::Base64;
use golem_common::model::RetryConfig;
use golem_common::tracing::TracingConfig;
use golem_common::{grpc_uri, SafeDisplay};
use golem_service_base::clients::registry::GrpcRegistryServiceConfig;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use golem_service_base::service::compiled_component::CompiledComponentServiceConfig;
use http::Uri;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The shared global Golem executor configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GolemConfig {
    pub tracing: TracingConfig,
    pub tracing_file_name_with_port: bool,
    pub key_value_storage: KeyValueStorageConfig,
    pub indexed_storage: IndexedStorageConfig,
    pub blob_storage: BlobStorageConfig,
    pub limits: Limits,
    pub retry: RetryConfig,
    pub compiled_component_service: CompiledComponentServiceConfig,
    pub shard_manager_service: ShardManagerServiceConfig,
    pub oplog: OplogConfig,
    pub suspend: SuspendConfig,
    pub active_workers: ActiveWorkersConfig,
    pub scheduler: SchedulerConfig,
    pub public_worker_api: WorkerServiceGrpcConfig,
    pub memory: MemoryConfig,
    pub rdbms: RdbmsConfig,
    pub resource_limits: ResourceLimitsConfig,
    pub component_cache: ComponentCacheConfig,
    pub agent_types_service: AgentTypesServiceConfig,
    pub agent_deployments_service: AgentDeploymentsServiceConfig,
    pub agent_webhooks_service: AgentWebhooksServiceConfig,
    pub registry_service: GrpcRegistryServiceConfig,
    pub engine: EngineConfig,
    pub grpc: GrpcApiConfig,
    pub http_address: String,
    pub http_port: u16,
}

impl SafeDisplay for GolemConfig {
    fn to_safe_string(&self) -> String {
        use std::fmt::Write;

        let mut result = String::new();

        let _ = writeln!(&mut result, "tracing:");
        let _ = writeln!(&mut result, "{}", self.tracing.to_safe_string_indented());
        let _ = writeln!(
            &mut result,
            "tracing file name with port: {}",
            self.tracing_file_name_with_port
        );
        let _ = writeln!(&mut result, "key-value storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.key_value_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "indexed storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.indexed_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "blob storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.blob_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "limits:");
        let _ = writeln!(&mut result, "{}", self.limits.to_safe_string_indented());
        let _ = writeln!(&mut result, "retry:");
        let _ = writeln!(&mut result, "{}", self.retry.to_safe_string_indented());
        let _ = writeln!(&mut result, "compiled component service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.compiled_component_service.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "shard manager service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.shard_manager_service.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "oplog:");
        let _ = writeln!(&mut result, "{}", self.oplog.to_safe_string_indented());
        let _ = writeln!(&mut result, "suspend:");
        let _ = writeln!(&mut result, "{}", self.suspend.to_safe_string_indented());
        let _ = writeln!(&mut result, "active_workers:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.active_workers.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "scheduler:");
        let _ = writeln!(&mut result, "{}", self.scheduler.to_safe_string_indented());
        let _ = writeln!(&mut result, "public worker api:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.public_worker_api.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "memory:");
        let _ = writeln!(&mut result, "{}", self.memory.to_safe_string_indented());
        let _ = writeln!(&mut result, "rdbms:");
        let _ = writeln!(&mut result, "{}", self.rdbms.to_safe_string_indented());
        let _ = writeln!(&mut result, "resource limits:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.resource_limits.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "registry service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.registry_service.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "component cache:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.component_cache.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "project service:");
        let _ = writeln!(&mut result, "agent types service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.agent_types_service.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "agent deployments service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.agent_deployments_service.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "agent webhooks service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.agent_webhooks_service.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "engine:");
        let _ = writeln!(&mut result, "{}", self.engine.to_safe_string_indented());

        let _ = writeln!(&mut result, "grpc:");
        let _ = writeln!(&mut result, "{}", self.grpc.to_safe_string_indented());

        let _ = writeln!(&mut result, "HTTP address: {}", self.http_address);
        let _ = writeln!(&mut result, "HTTP port: {}", self.http_port);

        result
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
            compiled_component_service: CompiledComponentServiceConfig::default(),
            shard_manager_service: ShardManagerServiceConfig::default(),
            oplog: OplogConfig::default(),
            suspend: SuspendConfig::default(),
            scheduler: SchedulerConfig::default(),
            active_workers: ActiveWorkersConfig::default(),
            public_worker_api: WorkerServiceGrpcConfig::default(),
            memory: MemoryConfig::default(),
            rdbms: RdbmsConfig::default(),
            resource_limits: ResourceLimitsConfig::default(),
            component_cache: ComponentCacheConfig::default(),
            agent_types_service: AgentTypesServiceConfig::default(),
            agent_deployments_service: AgentDeploymentsServiceConfig::default(),
            agent_webhooks_service: AgentWebhooksServiceConfig::default(),
            registry_service: GrpcRegistryServiceConfig::default(),
            engine: EngineConfig::default(),
            grpc: GrpcApiConfig::default(),
            http_address: "0.0.0.0".to_string(),
            http_port: 8082,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limits {
    pub max_active_workers: usize,
    pub invocation_result_broadcast_capacity: usize,
    pub max_concurrent_streams: u32,
    pub event_broadcast_capacity: usize,
    pub event_history_size: usize,
    pub fuel_to_borrow: u64,
    #[serde(with = "humantime_serde")]
    pub epoch_interval: Duration,
    pub epoch_ticks: u64,
    pub max_oplog_query_pages_size: usize,
}

impl SafeDisplay for Limits {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(
            &mut result,
            "max active workers: {}",
            self.max_active_workers
        );
        let _ = writeln!(
            &mut result,
            "invocation result broadcast capacity: {}",
            self.invocation_result_broadcast_capacity
        );
        let _ = writeln!(
            &mut result,
            "max concurrent streams: {}",
            self.max_concurrent_streams
        );
        let _ = writeln!(
            &mut result,
            "event broadcast capacity: {}",
            self.event_broadcast_capacity
        );
        let _ = writeln!(
            &mut result,
            "event history size: {}",
            self.event_history_size
        );
        let _ = writeln!(&mut result, "fuel to borrow: {}", self.fuel_to_borrow);
        let _ = writeln!(
            &mut result,
            "epoch interval: {}",
            self.epoch_interval.as_secs()
        );
        let _ = writeln!(&mut result, "epoch ticks: {}", self.epoch_ticks);
        let _ = writeln!(
            &mut result,
            "max oplog query pages: {}",
            self.max_oplog_query_pages_size
        );

        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrpcApiConfig {
    pub port: u16,
    pub tls: GrpcServerTlsConfig,
}

impl SafeDisplay for GrpcApiConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "port: {}", self.port);

        let _ = writeln!(&mut result, "tls:");
        let _ = writeln!(&mut result, "{}", self.tls.to_safe_string_indented());

        result
    }
}

impl Default for GrpcApiConfig {
    fn default() -> Self {
        Self {
            port: 9093,
            tls: GrpcServerTlsConfig::disabled(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ShardManagerServiceConfig {
    Grpc(Box<ShardManagerServiceGrpcConfig>),
    SingleShard(ShardManagerServiceSingleShardConfig),
}

impl SafeDisplay for ShardManagerServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            ShardManagerServiceConfig::Grpc(grpc) => {
                let _ = writeln!(&mut result, "grpc:");
                let _ = writeln!(&mut result, "{}", grpc.to_safe_string_indented());
            }
            ShardManagerServiceConfig::SingleShard(_) => {
                let _ = writeln!(&mut result, "single shard");
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardManagerServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    pub retries: RetryConfig,
    #[serde(flatten)]
    pub client_config: GrpcClientConfig,
}

impl ShardManagerServiceGrpcConfig {
    pub fn uri(&self) -> Uri {
        grpc_uri(&self.host, self.port, self.client_config.tls_enabled())
    }
}

impl SafeDisplay for ShardManagerServiceGrpcConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);

        let _ = writeln!(&mut result, "retries:");
        let _ = writeln!(&mut result, "{}", self.retries.to_safe_string_indented());

        let _ = writeln!(&mut result, "{}", self.client_config.to_safe_string());
        result
    }
}

impl Default for ShardManagerServiceGrpcConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9002,
            retries: RetryConfig::default(),
            client_config: GrpcClientConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardManagerServiceSingleShardConfig {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerServiceGrpcConfig {
    pub host: String,
    pub port: u16,
    #[serde(flatten)]
    pub client_config: GrpcClientConfig,
}

impl WorkerServiceGrpcConfig {
    pub fn uri(&self) -> Uri {
        grpc_uri(&self.host, self.port, self.client_config.tls_enabled())
    }
}

impl SafeDisplay for WorkerServiceGrpcConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "{}", self.client_config.to_safe_string());
        result
    }
}

impl Default for WorkerServiceGrpcConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9007,
            client_config: GrpcClientConfig {
                retries_on_unavailable: RetryConfig::max_attempts_5(),
                connect_timeout: Duration::from_secs(10),
                ..Default::default()
            },
        }
    }
}

impl GolemConfig {
    pub fn from_file(path: &str) -> Self {
        Figment::new()
            .merge(Toml::file(path))
            .extract()
            .expect("Failed to parse config")
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
                        Some(format!("{}.{}.{}", elems[0], self.grpc.port, elems[1]))
                    } else {
                        Some(format!("{}.{}", file_name, self.grpc.port))
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SuspendConfig {
    #[serde(with = "humantime_serde")]
    pub suspend_after: Duration,
}

impl SafeDisplay for SuspendConfig {
    fn to_safe_string(&self) -> String {
        format!("suspend after: {:?}", self.suspend_after)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActiveWorkersConfig {
    pub drop_when_full: f64,
    #[serde(with = "humantime_serde")]
    pub ttl: Duration,
}

impl SafeDisplay for ActiveWorkersConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "drop when full: {}", self.drop_when_full);
        let _ = writeln!(&mut result, "ttl: {:?}", self.ttl);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulerConfig {
    #[serde(with = "humantime_serde")]
    pub refresh_interval: Duration,
}

impl SafeDisplay for SchedulerConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "refresh interval: {:?}", self.refresh_interval);
        result
    }
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
    pub default_snapshotting: DefaultSnapshottingConfig,
}

impl SafeDisplay for OplogConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "max operations before commit: {}",
            self.max_operations_before_commit
        );
        let _ = writeln!(
            &mut result,
            "max operations before commit for ephemerals: {}",
            self.max_operations_before_commit_ephemeral
        );
        let _ = writeln!(&mut result, "max payload size: {}", self.max_payload_size);
        let _ = writeln!(
            &mut result,
            "indexed storage layers: {}",
            self.indexed_storage_layers
        );
        let _ = writeln!(
            &mut result,
            "blob storage layers: {}",
            self.blob_storage_layers
        );
        let _ = writeln!(&mut result, "entry count limit: {}", self.entry_count_limit);
        let _ = writeln!(&mut result, "archive interval: {:?}", self.archive_interval);
        let _ = writeln!(&mut result, "default snapshotting:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.default_snapshotting.to_safe_string_indented()
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum KeyValueStorageConfig {
    Redis(RedisConfig),
    Sqlite(DbSqliteConfig),
    MultiSqlite(KeyValueStorageMultiSqliteConfig),
    InMemory(KeyValueStorageInMemoryConfig),
}

impl SafeDisplay for KeyValueStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            KeyValueStorageConfig::Redis(inner) => {
                let _ = writeln!(&mut result, "redis:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageConfig::Sqlite(inner) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageConfig::MultiSqlite(inner) => {
                let _ = writeln!(&mut result, "multi-sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            KeyValueStorageConfig::InMemory(inner) => {
                let _ = writeln!(&mut result, "in-memory:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyValueStorageMultiSqliteConfig {
    pub root_dir: PathBuf,
    pub max_connections: u32,
    pub foreign_keys: bool,
}

impl SafeDisplay for KeyValueStorageMultiSqliteConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "root dir: {}", self.root_dir.display());
        let _ = writeln!(&mut result, "max connections: {}", self.max_connections);
        let _ = writeln!(&mut result, "foreign keys: {}", self.foreign_keys);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyValueStorageInMemoryConfig {}

impl SafeDisplay for KeyValueStorageInMemoryConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum IndexedStorageConfig {
    KVStoreRedis(IndexedStorageKVStoreRedisConfig),
    Redis(RedisConfig),
    KVStoreSqlite(IndexedStorageKVStoreSqliteConfig),
    KVStoreMultiSqlite(IndexedStorageKVStoreMultiSqliteConfig),
    Sqlite(DbSqliteConfig),
    MultiSqlite(IndexedStorageMultiSqliteConfig),
    InMemory(IndexedStorageInMemoryConfig),
}

impl SafeDisplay for IndexedStorageConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            IndexedStorageConfig::KVStoreRedis(inner) => {
                let _ = writeln!(&mut result, "redis kv-store:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::Redis(inner) => {
                let _ = writeln!(&mut result, "redis:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::KVStoreSqlite(inner) => {
                let _ = writeln!(&mut result, "sqlite kv-store:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::KVStoreMultiSqlite(inner) => {
                let _ = writeln!(&mut result, "multi-sqlite kv-store:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::Sqlite(inner) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::MultiSqlite(inner) => {
                let _ = writeln!(&mut result, "multi-sqlite:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            IndexedStorageConfig::InMemory(inner) => {
                let _ = writeln!(&mut result, "in-memory:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStorageKVStoreRedisConfig {}

impl SafeDisplay for IndexedStorageKVStoreRedisConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStorageKVStoreSqliteConfig {}

impl SafeDisplay for IndexedStorageKVStoreSqliteConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStorageKVStoreMultiSqliteConfig {}

impl SafeDisplay for IndexedStorageKVStoreMultiSqliteConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStorageMultiSqliteConfig {
    pub root_dir: PathBuf,
    pub max_connections: u32,
    pub foreign_keys: bool,
}

impl SafeDisplay for IndexedStorageMultiSqliteConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "root dir: {}", self.root_dir.display());
        let _ = writeln!(&mut result, "max connections: {}", self.max_connections);
        let _ = writeln!(&mut result, "foreign keys: {}", self.foreign_keys);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedStorageInMemoryConfig {}

impl SafeDisplay for IndexedStorageInMemoryConfig {
    fn to_safe_string(&self) -> String {
        "".to_string()
    }
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

impl SafeDisplay for MemoryConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        if let Some(ovrd) = &self.system_memory_override {
            let _ = writeln!(&mut result, "system memory override: {ovrd}");
        }
        let _ = writeln!(
            &mut result,
            "worker memory ratio: {}",
            self.worker_memory_ratio
        );
        let _ = writeln!(
            &mut result,
            "worker estimate coefficient: {}",
            self.worker_estimate_coefficient
        );
        let _ = writeln!(
            &mut result,
            "acquire retry delay: {:?}",
            self.acquire_retry_delay
        );
        let _ = writeln!(&mut result, "oom retry config:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.oom_retry_config.to_safe_string_indented()
        );

        result
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct RdbmsConfig {
    pub pool: RdbmsPoolConfig,
    pub query: RdbmsQueryConfig,
}

impl SafeDisplay for RdbmsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "pool:");
        let _ = writeln!(&mut result, "{}", self.pool.to_safe_string_indented());
        let _ = writeln!(&mut result, "query:");
        let _ = writeln!(&mut result, "{}", self.query.to_safe_string_indented());
        result
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RdbmsQueryConfig {
    pub query_batch: usize,
}

impl SafeDisplay for RdbmsQueryConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "batch size: {}", self.query_batch);
        result
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct RdbmsPoolConfig {
    pub max_connections: u32,
    #[serde(with = "humantime_serde")]
    pub eviction_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub eviction_period: Duration,
    #[serde(with = "humantime_serde")]
    pub acquire_timeout: Duration,
}

impl SafeDisplay for RdbmsPoolConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "max connections: {}", self.max_connections);
        let _ = writeln!(&mut result, "eviction ttl: {:?}", self.eviction_ttl);
        let _ = writeln!(&mut result, "eviction period: {:?}", self.eviction_period);
        let _ = writeln!(&mut result, "acquire timeout: {:?}", self.acquire_timeout);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum ResourceLimitsConfig {
    Grpc(ResourceLimitsGrpcConfig),
    Disabled(ResourceLimitsDisabledConfig),
}

impl SafeDisplay for ResourceLimitsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            ResourceLimitsConfig::Grpc(grpc) => {
                let _ = writeln!(&mut result, "grpc:");
                let _ = writeln!(&mut result, "{}", grpc.to_safe_string_indented());
            }
            ResourceLimitsConfig::Disabled(_) => {
                let _ = writeln!(&mut result, "disabled");
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceLimitsGrpcConfig {
    #[serde(with = "humantime_serde")]
    pub batch_update_interval: Duration,
}

impl SafeDisplay for ResourceLimitsGrpcConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "batch update interval: {:?}",
            self.batch_update_interval
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceLimitsDisabledConfig {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentCacheConfig {
    pub max_capacity: usize,
    pub max_metadata_capacity: usize,
    pub max_resolved_component_capacity: usize,
    #[serde(with = "humantime_serde")]
    pub time_to_idle: Duration,
}

impl SafeDisplay for ComponentCacheConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "max capacity: {}", self.max_capacity);
        let _ = writeln!(
            &mut result,
            "max metadata capacity: {}",
            self.max_metadata_capacity
        );
        let _ = writeln!(
            &mut result,
            "max resolved component capacity: {}",
            self.max_resolved_component_capacity
        );
        let _ = writeln!(&mut result, "time to idle: {:?}", self.time_to_idle);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum AgentTypesServiceConfig {
    Grpc(AgentTypesServiceGrpcConfig),
    Local(AgentTypesServiceLocalConfig),
}

impl SafeDisplay for AgentTypesServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            AgentTypesServiceConfig::Grpc(grpc) => {
                let _ = writeln!(&mut result, "grpc:");
                let _ = writeln!(&mut result, "{}", grpc.to_safe_string_indented());
            }
            AgentTypesServiceConfig::Local(_) => {
                let _ = writeln!(&mut result, "local");
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentTypesServiceGrpcConfig {
    #[serde(with = "humantime_serde")]
    pub cache_time_to_idle: Duration,
}

impl SafeDisplay for AgentTypesServiceGrpcConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "cache time to idle: {:?}",
            self.cache_time_to_idle
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentTypesServiceLocalConfig {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentDeploymentsServiceConfig {
    pub cache_capacity: usize,
    pub cache_ttl: Duration,
    #[serde(with = "humantime_serde")]
    pub cache_eviction_interval: Duration,
}

impl Default for AgentDeploymentsServiceConfig {
    fn default() -> Self {
        Self {
            cache_capacity: 1000,
            cache_ttl: Duration::from_mins(5),
            cache_eviction_interval: Duration::from_mins(1),
        }
    }
}

impl SafeDisplay for AgentDeploymentsServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "cache_capacity: {}", self.cache_capacity);
        let _ = writeln!(&mut result, "cache_ttl: {:?}", self.cache_ttl);
        let _ = writeln!(
            &mut result,
            "cache_eviction_interval: {:?}",
            self.cache_eviction_interval
        );
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentWebhooksServiceConfig {
    pub use_https_for_webhook_url: bool,
    pub hmac_key: Base64,
}

impl Default for AgentWebhooksServiceConfig {
    fn default() -> Self {
        Self {
            use_https_for_webhook_url: true,
            hmac_key: Base64(vec![
                0x2b, 0x7e, 0x02, 0xa3, 0x8a, 0x51, 0x30, 0x39, 0x7b, 0x74, 0x1d, 0xdc, 0x60, 0x1f,
                0xb5, 0xfc, 0xdd, 0x09, 0xde, 0xd3, 0x33, 0x25, 0x62, 0x38, 0x17, 0x23, 0xcd, 0x3a,
                0xc9, 0x86, 0x1e, 0x41,
            ]),
        }
    }
}

impl SafeDisplay for AgentWebhooksServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "use_https_for_webhook_url: {:?}",
            self.use_https_for_webhook_url
        );
        let _ = writeln!(&mut result, "hmac_key: *******");
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum DefaultSnapshottingConfig {
    Disabled,
    Periodic(DefaultSnapshottingPeriodicConfig),
    EveryNInvocation(DefaultSnapshottingEveryNInvocationConfig),
}

impl Default for DefaultSnapshottingConfig {
    fn default() -> Self {
        Self::Disabled
    }
}

impl SafeDisplay for DefaultSnapshottingConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            DefaultSnapshottingConfig::Disabled => {
                let _ = writeln!(&mut result, "disabled");
            }
            DefaultSnapshottingConfig::Periodic(config) => {
                let _ = writeln!(&mut result, "periodic:");
                let _ = writeln!(&mut result, "{}", config.to_safe_string_indented());
            }
            DefaultSnapshottingConfig::EveryNInvocation(config) => {
                let _ = writeln!(&mut result, "every n invocation:");
                let _ = writeln!(&mut result, "{}", config.to_safe_string_indented());
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DefaultSnapshottingPeriodicConfig {
    #[serde(with = "humantime_serde")]
    pub period: Duration,
}

impl SafeDisplay for DefaultSnapshottingPeriodicConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "period: {:?}", self.period);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DefaultSnapshottingEveryNInvocationConfig {
    pub count: u16,
}

impl SafeDisplay for DefaultSnapshottingEveryNInvocationConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "count: {}", self.count);
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct EngineConfig {
    pub enable_fs_cache: bool,
}

impl SafeDisplay for EngineConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "enable fs cache: {}", self.enable_fs_cache);
        result
    }
}

impl HasConfigExamples<GolemConfig> for GolemConfig {
    fn examples() -> Vec<ConfigExample<GolemConfig>> {
        vec![
            (
                "with redis indexed_storage, s3 blob storage, single shard manager service",
                Self {
                    key_value_storage: KeyValueStorageConfig::InMemory(
                        KeyValueStorageInMemoryConfig {},
                    ),
                    indexed_storage: IndexedStorageConfig::Redis(RedisConfig::default()),
                    blob_storage: BlobStorageConfig::default_s3(),
                    shard_manager_service: ShardManagerServiceConfig::SingleShard(
                        ShardManagerServiceSingleShardConfig {},
                    ),
                    ..Self::default()
                },
            ),
            (
                "with in-memory key value storage, indexed storage and blob storage",
                Self {
                    key_value_storage: KeyValueStorageConfig::InMemory(
                        KeyValueStorageInMemoryConfig {},
                    ),
                    indexed_storage: IndexedStorageConfig::InMemory(
                        IndexedStorageInMemoryConfig {},
                    ),
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
            event_broadcast_capacity: 1024,
            event_history_size: 128,
            fuel_to_borrow: 10000,
            epoch_interval: Duration::from_millis(10),
            epoch_ticks: 1,
            max_oplog_query_pages_size: 100,
        }
    }
}

impl Default for ShardManagerServiceConfig {
    fn default() -> Self {
        Self::Grpc(Box::default())
    }
}

impl Default for OplogConfig {
    fn default() -> Self {
        Self {
            max_operations_before_commit: 128,
            max_operations_before_commit_ephemeral: 1024,
            max_payload_size: 64 * 1024,
            indexed_storage_layers: 2,
            blob_storage_layers: 1,
            entry_count_limit: 1024,
            archive_interval: Duration::from_secs(60 * 60 * 24), // 24 hours
            default_snapshotting: DefaultSnapshottingConfig::default(),
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
        Self::KVStoreRedis(IndexedStorageKVStoreRedisConfig {})
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

impl Default for RdbmsQueryConfig {
    fn default() -> Self {
        Self { query_batch: 50 }
    }
}

impl Default for RdbmsPoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 20,
            eviction_ttl: Duration::from_secs(10 * 60),
            eviction_period: Duration::from_secs(2 * 60),
            acquire_timeout: Duration::from_secs(3),
        }
    }
}

impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        Self::Grpc(ResourceLimitsGrpcConfig {
            batch_update_interval: Duration::from_secs(60),
        })
    }
}

impl Default for ComponentCacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: 32,
            max_metadata_capacity: 16384,
            max_resolved_component_capacity: 1024,
            time_to_idle: Duration::from_secs(12 * 60 * 60),
        }
    }
}

impl Default for AgentTypesServiceConfig {
    fn default() -> Self {
        Self::Grpc(AgentTypesServiceGrpcConfig::default())
    }
}

impl Default for AgentTypesServiceGrpcConfig {
    fn default() -> Self {
        Self {
            cache_time_to_idle: Duration::from_secs(60),
        }
    }
}

pub fn make_config_loader() -> ConfigLoader<GolemConfig> {
    ConfigLoader::new_with_examples(Path::new("config/worker-executor.toml"))
}
