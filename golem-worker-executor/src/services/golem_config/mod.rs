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

use figment::Figment;
use figment::providers::{Format, Toml};
use golem_common::SafeDisplay;
use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples, RedisConfig};
use golem_common::model::RetryConfig;
use golem_common::tracing::TracingConfig;
use golem_service_base::clients::registry::GrpcRegistryServiceConfig;
use golem_service_base::clients::shard_manager::{GrpcShardManagerConfig, canonical_advertised_ip};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::grpc::client::GrpcClientConfig;
use golem_service_base::service::compiled_component::CompiledComponentServiceConfig;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

mod filesystem;
pub use filesystem::{
    FilesystemObjectLimitPolicyConfig, FilesystemPressureConfig, FilesystemStorageConfig,
    FilesystemStorageMode, FilesystemStorageModeKind,
};
mod filesystem_snapshots;
pub(crate) use filesystem_snapshots::DEFAULT_FILESYSTEM_SNAPSHOT_CAPTURE_WAIT;
#[cfg(test)]
pub(crate) use filesystem_snapshots::default_filesystem_snapshot_storage_retry;
pub use filesystem_snapshots::{
    DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE, FilesystemSnapshotRepositoryKey,
    FilesystemSnapshotStoreConfig, FilesystemSnapshotUploadConfig, FilesystemSnapshotUploadValues,
    FilesystemSnapshotsConfig, FilesystemSnapshotsDisabledConfig, FilesystemSnapshotsKind,
};
mod network;
pub use network::{
    AgentSocketsConfig, GrpcApiConfig, HttpClientConfig, HttpClientDisabledConfig,
    HttpClientEnabledConfig, WorkerServiceGrpcConfig,
};
mod oplog;
pub use oplog::{OplogConfig, OplogSweepConfig, SnapshotPolicy};
mod resources;
pub use resources::{
    MemoryConfig, QuotaServiceConfig, ResourceLimitsConfig, ResourceLimitsDisabledConfig,
    ResourceLimitsGrpcConfig, ResourceUsageMeteringConfig,
};
mod runtime;
pub use runtime::{
    ActiveAgentsConfig, AgentStatusCheckpointConfig, AgentStatusFlushConfig, DurableStreamConfig,
    EngineConfig, InvocationResultsConfig, Limits, ReadOnlyCacheConfig, SchedulerConfig,
    SuspendConfig,
};
mod services;
pub use services::{
    AgentTypesServiceConfig, AgentTypesServiceGrpcConfig, AgentTypesServiceLocalConfig,
    AgentWebhooksServiceConfig, ComponentCacheConfig, DirectInvocationAuthCacheConfig,
    EnvironmentStateServiceConfig, RdbmsConfig, RdbmsPoolConfig, RdbmsQueryConfig,
};
mod storage;
pub use storage::{
    IndexedStorageConfig, IndexedStorageInMemoryConfig, IndexedStorageKVStoreMultiSqliteConfig,
    IndexedStorageKVStoreRedisConfig, IndexedStorageKVStoreSqliteConfig,
    IndexedStorageMultiSqliteConfig, IndexedStoragePostgresConfig, KeyValueStorageConfig,
    KeyValueStorageInMemoryConfig, KeyValueStorageInnerConfig, KeyValueStorageMultiSqliteConfig,
    KeyValueStorageNamespaceRoutedConfig, KeyValueStoragePostgresConfig, SchedulerStorageConfig,
    SchedulerStorageInMemoryConfig, SchedulerStoragePostgresConfig,
    default_key_value_storage_retry,
};
use storage::{default_indexed_storage_retry, default_scheduler_storage_retry};
#[cfg(test)]
mod tests;

/// The shared global Golem executor configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GolemConfig {
    pub tracing: TracingConfig,
    pub tracing_file_name_with_port: bool,
    pub key_value_storage: KeyValueStorageConfig,
    /// Retry policy applied to every key-value storage operation whose failure is classified as
    /// transient, whichever backend serves it. Without it a brief backend outage - a connection
    /// pool acquisition timeout, a dropped connection - surfaces to hot paths such as promise and
    /// agent status updates as a hard failure.
    #[serde(default = "default_key_value_storage_retry")]
    pub key_value_storage_retry: RetryConfig,
    /// Retry policy applied when scheduling or cancelling hits a briefly exhausted connection
    /// pool (a pool acquisition timeout). Those two panic once it is exhausted, because their
    /// entry points return no error for a caller to act on; the tick's own storage calls
    /// propagate their errors instead.
    #[serde(default = "default_scheduler_storage_retry")]
    pub scheduler_storage_retry: RetryConfig,
    /// Retry policy applied to SQL-backed indexed storage (oplog) operations when the connection
    /// pool is briefly exhausted (a pool acquisition timeout). Transient failures are retried so
    /// oplog commits do not crash the executor under load.
    #[serde(default = "default_indexed_storage_retry")]
    pub indexed_storage_retry: RetryConfig,
    pub scheduler_storage: SchedulerStorageConfig,
    pub indexed_storage: IndexedStorageConfig,
    pub blob_storage: BlobStorageConfig,
    pub limits: Limits,
    #[serde(default)]
    pub durable_stream: DurableStreamConfig,
    pub retry: RetryConfig,
    #[serde(with = "humantime_serde")]
    pub max_in_function_retry_delay: Duration,
    pub compiled_component_service: CompiledComponentServiceConfig,
    pub shard_manager: GrpcShardManagerConfig,
    pub oplog: OplogConfig,
    pub suspend: SuspendConfig,
    pub active_agents: ActiveAgentsConfig,
    #[serde(default)]
    pub agent_status_flush: AgentStatusFlushConfig,
    #[serde(default)]
    pub agent_status_checkpoint: AgentStatusCheckpointConfig,
    #[serde(default)]
    pub invocation_results: InvocationResultsConfig,
    pub scheduler: SchedulerConfig,
    pub public_worker_api: WorkerServiceGrpcConfig,
    pub memory: MemoryConfig,
    pub filesystem_storage: FilesystemStorageConfig,
    #[serde(default)]
    pub filesystem_snapshots: FilesystemSnapshotsConfig,
    #[serde(default)]
    pub resource_usage_metering: ResourceUsageMeteringConfig,
    pub rdbms: RdbmsConfig,
    pub resource_limits: ResourceLimitsConfig,
    pub component_cache: ComponentCacheConfig,
    pub agent_types_service: AgentTypesServiceConfig,
    pub environment_state_service: EnvironmentStateServiceConfig,
    pub direct_invocation_auth_cache: DirectInvocationAuthCacheConfig,
    #[serde(default)]
    pub read_only_cache: ReadOnlyCacheConfig,
    pub agent_webhooks_service: AgentWebhooksServiceConfig,
    pub registry_service: GrpcRegistryServiceConfig,
    pub quota_service: QuotaServiceConfig,
    pub engine: EngineConfig,
    pub grpc: GrpcApiConfig,
    pub http_client: HttpClientConfig,
    pub mcp_transport: golem_mcp_import::transport::Limits,
    pub max_websocket_connections: usize,
    pub agent_sockets: AgentSocketsConfig,
    /// The address every listener of this service binds to. `0.0.0.0` accepts IPv4 only; `::`
    /// accepts both IPv6 and IPv4.
    pub bind_address: IpAddr,
    pub http_port: u16,
    /// How often runtime/allocator metrics are sampled and refreshed into the
    /// gauges exposed on `/metrics`. Prometheus scrapes the rendered values
    /// independently; this is the in-process resolution.
    #[serde(with = "humantime_serde")]
    pub runtime_metrics_sampling_interval: Duration,
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
        let _ = writeln!(&mut result, "key-value storage retry:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.key_value_storage_retry.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "scheduler storage retry:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.scheduler_storage_retry.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "indexed storage retry:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.indexed_storage_retry.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "scheduler storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.scheduler_storage.to_safe_string_indented()
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
        let _ = writeln!(&mut result, "durable stream:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.durable_stream.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "retry:");
        let _ = writeln!(&mut result, "{}", self.retry.to_safe_string_indented());
        let _ = writeln!(
            &mut result,
            "max in-function retry delay: {}s",
            self.max_in_function_retry_delay.as_secs()
        );
        let _ = writeln!(&mut result, "compiled component service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.compiled_component_service.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "shard manager:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.shard_manager.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "oplog:");
        let _ = writeln!(&mut result, "{}", self.oplog.to_safe_string_indented());
        let _ = writeln!(&mut result, "suspend:");
        let _ = writeln!(&mut result, "{}", self.suspend.to_safe_string_indented());
        let _ = writeln!(&mut result, "active_agents:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.active_agents.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "agent_status_flush:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.agent_status_flush.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "agent_status_checkpoint:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.agent_status_checkpoint.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "invocation_results:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.invocation_results.to_safe_string_indented()
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
        let _ = writeln!(&mut result, "filesystem storage:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.filesystem_storage.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "filesystem snapshots:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.filesystem_snapshots.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "resource usage metering:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.resource_usage_metering.to_safe_string_indented()
        );
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
        let _ = writeln!(&mut result, "quota service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.quota_service.to_safe_string_indented()
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

        let _ = writeln!(&mut result, "environment state service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.environment_state_service.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "direct invocation auth cache:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.direct_invocation_auth_cache.to_safe_string_indented()
        );

        let _ = writeln!(&mut result, "read only cache:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.read_only_cache.to_safe_string_indented()
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

        let _ = writeln!(&mut result, "http client:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.http_client.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "MCP transport: {:?}", self.mcp_transport);

        let _ = writeln!(
            &mut result,
            "max websocket connections: {}",
            self.max_websocket_connections
        );
        let _ = writeln!(&mut result, "agent sockets:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.agent_sockets.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "bind address: {}", self.bind_address);
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
            key_value_storage_retry: default_key_value_storage_retry(),
            scheduler_storage_retry: default_scheduler_storage_retry(),
            indexed_storage_retry: default_indexed_storage_retry(),
            scheduler_storage: SchedulerStorageConfig::default(),
            indexed_storage: IndexedStorageConfig::default(),
            blob_storage: BlobStorageConfig::default(),
            limits: Limits::default(),
            durable_stream: DurableStreamConfig::default(),
            retry: RetryConfig::max_attempts_3(),
            max_in_function_retry_delay: Duration::from_secs(20),
            compiled_component_service: CompiledComponentServiceConfig::default(),
            shard_manager: GrpcShardManagerConfig::default(),
            oplog: OplogConfig::default(),
            suspend: SuspendConfig::default(),
            scheduler: SchedulerConfig::default(),
            active_agents: ActiveAgentsConfig::default(),
            agent_status_flush: AgentStatusFlushConfig::default(),
            agent_status_checkpoint: AgentStatusCheckpointConfig::default(),
            invocation_results: InvocationResultsConfig::default(),
            public_worker_api: WorkerServiceGrpcConfig::default(),
            memory: MemoryConfig::default(),
            filesystem_storage: FilesystemStorageConfig::default(),
            filesystem_snapshots: FilesystemSnapshotsConfig::default(),
            resource_usage_metering: ResourceUsageMeteringConfig::default(),
            rdbms: RdbmsConfig::default(),
            resource_limits: ResourceLimitsConfig::default(),
            component_cache: ComponentCacheConfig::default(),
            agent_types_service: AgentTypesServiceConfig::default(),
            environment_state_service: EnvironmentStateServiceConfig::default(),
            direct_invocation_auth_cache: DirectInvocationAuthCacheConfig::default(),
            read_only_cache: ReadOnlyCacheConfig::default(),
            agent_webhooks_service: AgentWebhooksServiceConfig::default(),
            registry_service: {
                let default = GrpcRegistryServiceConfig::default();
                GrpcRegistryServiceConfig {
                    client_config: GrpcClientConfig {
                        request_timeout: Some(Duration::from_secs(30)),
                        ..default.client_config
                    },
                    ..default
                }
            },
            quota_service: QuotaServiceConfig::default(),
            engine: EngineConfig::default(),
            grpc: GrpcApiConfig::default(),
            http_client: HttpClientConfig::default(),
            mcp_transport: golem_mcp_import::transport::Limits::default(),
            max_websocket_connections: 100,
            agent_sockets: AgentSocketsConfig::default(),
            bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            http_port: 8082,
            runtime_metrics_sampling_interval: Duration::from_secs(5),
        }
    }
}

impl GolemConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.blob_storage.validate()?;
        self.mcp_transport.validate().map_err(anyhow::Error::msg)?;
        self.durable_stream.validate()?;
        self.invocation_results.validate()?;
        anyhow::ensure!(
            (1..=tokio::sync::Semaphore::MAX_PERMITS)
                .contains(&self.limits.max_concurrent_oplog_entry_reads),
            "limits.max_concurrent_oplog_entry_reads must be between 1 and {}",
            tokio::sync::Semaphore::MAX_PERMITS
        );
        if let Some(advertised_ip) = self.grpc.advertised_ip {
            let advertised_ip = canonical_advertised_ip(advertised_ip)
                .map_err(|error| anyhow::anyhow!("grpc.advertised_ip: {error}"))?;
            let bind_address = self.bind_address.to_canonical();
            anyhow::ensure!(
                !(advertised_ip.is_ipv6() && bind_address.is_ipv4()),
                "grpc.advertised_ip {advertised_ip} is an IPv6 address, but bind_address {bind_address} accepts IPv4 only; set bind_address to \"::\""
            );
            anyhow::ensure!(
                !(advertised_ip.is_ipv4()
                    && bind_address.is_ipv6()
                    && !bind_address.is_unspecified()),
                "grpc.advertised_ip {advertised_ip} is an IPv4 address, but bind_address {bind_address} accepts IPv6 only"
            );
        }
        Ok(())
    }
}

impl GolemConfig {
    pub fn from_file(path: &str) -> Self {
        Figment::new()
            .merge(Toml::file(path))
            .extract()
            .expect("Failed to parse config")
    }

    pub fn http_addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind_address, self.http_port)
    }

    pub fn grpc_addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind_address, self.grpc.port)
    }

    pub fn add_port_to_tracing_file_name_if_enabled(&mut self) {
        if self.tracing_file_name_with_port
            && let Some(file_name) = &self.tracing.file_name
        {
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

impl GolemConfig {
    pub(crate) fn effective_resource_usage_metering(&self) -> ResourceUsageMeteringConfig {
        ResourceUsageMeteringConfig {
            filesystem: self.resource_usage_metering.filesystem
                && matches!(
                    self.filesystem_storage.mode,
                    FilesystemStorageMode::ManagedXfs { .. }
                ),
            ..self.resource_usage_metering
        }
    }
}

impl HasConfigExamples<GolemConfig> for GolemConfig {
    fn examples() -> Vec<ConfigExample<GolemConfig>> {
        vec![
            (
                "with redis indexed_storage and s3 blob storage",
                Self {
                    key_value_storage: KeyValueStorageConfig::InMemory(
                        KeyValueStorageInMemoryConfig {},
                    ),
                    indexed_storage: IndexedStorageConfig::Redis(RedisConfig::default()),
                    blob_storage: BlobStorageConfig::default_s3(),
                    ..Self::default()
                },
            ),
            (
                "with dual-stack listeners and an advertised executor address",
                Self {
                    bind_address: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                    grpc: GrpcApiConfig {
                        advertised_ip: Some(IpAddr::V6(Ipv6Addr::new(0xfdaa, 0, 0, 0, 0, 0, 0, 2))),
                        ..GrpcApiConfig::default()
                    },
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

pub fn make_config_loader() -> ConfigLoader<GolemConfig> {
    ConfigLoader::new_with_examples(Path::new("config/worker-executor.toml"))
}
