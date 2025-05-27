use cloud_common::config::RemoteCloudServiceConfig;
use cloud_worker_executor::services::config::{
    CloudComponentCacheConfig, CloudComponentServiceConfig,
};
use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_common::model::RetryConfig;
use golem_common::tracing::TracingConfig;
use golem_service_base::config::BlobStorageConfig;
use golem_worker_executor_base::services::golem_config::{
    ActiveWorkersConfig, CompiledComponentServiceConfig, GolemConfig, IndexedStorageConfig,
    KeyValueStorageConfig, Limits, MemoryConfig, OplogConfig, PluginServiceConfig, RdbmsConfig,
    SchedulerConfig, ShardManagerServiceConfig, ShardManagerServiceSingleShardConfig,
    SuspendConfig, WorkerServiceGrpcConfig,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// A wrapper over golem config with a few custom behaviour
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DebugConfig {
    // inherited from regular worker-executor config
    pub tracing: TracingConfig,
    pub tracing_file_name_with_port: bool,
    pub key_value_storage: KeyValueStorageConfig,
    pub indexed_storage: IndexedStorageConfig,
    pub blob_storage: BlobStorageConfig,
    pub limits: Limits,
    pub retry: RetryConfig,
    pub compiled_component_service: CompiledComponentServiceConfig,
    pub plugin_service: PluginServiceConfig,
    pub oplog: OplogConfig,
    pub suspend: SuspendConfig,
    pub active_workers: ActiveWorkersConfig,
    pub scheduler: SchedulerConfig,
    pub public_worker_api: WorkerServiceGrpcConfig,
    pub memory: MemoryConfig,
    pub rdbms: RdbmsConfig,
    pub grpc_address: String,
    pub port: u16,
    pub http_address: String,
    pub http_port: u16,

    // debug service specific fields
    pub cloud_service: RemoteCloudServiceConfig,
    pub component_service: CloudComponentServiceConfig,
    pub component_cache: CloudComponentCacheConfig,
}

impl DebugConfig {
    pub fn into_golem_config(self) -> GolemConfig {
        GolemConfig {
            tracing: self.tracing,
            tracing_file_name_with_port: self.tracing_file_name_with_port,
            key_value_storage: self.key_value_storage,
            indexed_storage: self.indexed_storage,
            blob_storage: self.blob_storage,
            limits: self.limits,
            retry: self.retry,
            compiled_component_service: self.compiled_component_service,
            plugin_service: self.plugin_service,
            oplog: self.oplog,
            suspend: self.suspend,
            active_workers: self.active_workers,
            scheduler: self.scheduler,
            public_worker_api: self.public_worker_api,
            memory: self.memory,
            rdbms: self.rdbms,
            grpc_address: self.grpc_address,
            port: self.port,
            http_address: self.http_address,
            http_port: self.http_port,
            shard_manager_service: ShardManagerServiceConfig::SingleShard(
                ShardManagerServiceSingleShardConfig {},
            ),
        }
    }
}

impl Default for DebugConfig {
    fn default() -> Self {
        let default_golem_config = GolemConfig::default();
        Self {
            tracing: default_golem_config.tracing,
            tracing_file_name_with_port: default_golem_config.tracing_file_name_with_port,
            key_value_storage: default_golem_config.key_value_storage,
            indexed_storage: default_golem_config.indexed_storage,
            blob_storage: default_golem_config.blob_storage,
            limits: default_golem_config.limits,
            retry: default_golem_config.retry,
            compiled_component_service: default_golem_config.compiled_component_service,
            plugin_service: default_golem_config.plugin_service,
            oplog: default_golem_config.oplog,
            suspend: default_golem_config.suspend,
            active_workers: default_golem_config.active_workers,
            scheduler: default_golem_config.scheduler,
            public_worker_api: default_golem_config.public_worker_api,
            memory: default_golem_config.memory,
            rdbms: default_golem_config.rdbms,
            grpc_address: default_golem_config.grpc_address,
            port: default_golem_config.port,
            http_address: default_golem_config.http_address,
            http_port: default_golem_config.http_port,
            cloud_service: RemoteCloudServiceConfig::default(),
            component_cache: CloudComponentCacheConfig::default(),
            component_service: CloudComponentServiceConfig::default(),
        }
    }
}

impl HasConfigExamples<DebugConfig> for DebugConfig {
    fn examples() -> Vec<ConfigExample<DebugConfig>> {
        vec![("default-debug-config", DebugConfig::default())]
    }
}

pub fn make_debug_config_loader() -> ConfigLoader<DebugConfig> {
    ConfigLoader::new_with_examples(&PathBuf::from("config/debug-worker-executor.toml"))
}
