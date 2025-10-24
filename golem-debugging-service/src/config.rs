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

use golem_common::config::{ConfigExample, ConfigLoader, HasConfigExamples};
use golem_common::model::RetryConfig;
use golem_common::tracing::TracingConfig;
use golem_common::SafeDisplay;
use golem_service_base::clients::RemoteServiceConfig;
use golem_service_base::config::BlobStorageConfig;
use golem_worker_executor::services::golem_config::{
    ActiveWorkersConfig, AgentTypesServiceConfig, CompiledComponentServiceConfig,
    ComponentCacheConfig, ComponentServiceConfig, ComponentServiceGrpcConfig, EngineConfig,
    GolemConfig, IndexedStorageConfig, KeyValueStorageConfig, Limits, MemoryConfig, OplogConfig,
    PluginServiceConfig, ProjectServiceConfig, RdbmsConfig, ResourceLimitsConfig, SchedulerConfig,
    ShardManagerServiceConfig, ShardManagerServiceSingleShardConfig, SuspendConfig,
    WorkerServiceGrpcConfig,
};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
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
    pub http_address: String,
    pub http_port: u16,
    pub component_service: ComponentServiceGrpcConfig,
    pub component_cache: ComponentCacheConfig,
    pub project_service: ProjectServiceConfig,
    pub agent_types_service: AgentTypesServiceConfig,
    pub engine: EngineConfig,
    pub resource_limits: ResourceLimitsConfig,

    // debug service specific fields
    pub cloud_service: RemoteServiceConfig,
    pub cors_origin_regex: String,
}

impl DebugConfig {
    pub fn into_golem_config(self) -> GolemConfig {
        let default_golem_config = GolemConfig::default();
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
            resource_limits: self.resource_limits,
            component_service: ComponentServiceConfig::Grpc(self.component_service),
            component_cache: self.component_cache,
            project_service: self.project_service,
            agent_types_service: self.agent_types_service,
            engine: self.engine,
            // unused
            grpc_address: default_golem_config.grpc_address,
            // unused
            port: default_golem_config.port,
            http_address: self.http_address,
            http_port: self.http_port,
            shard_manager_service: ShardManagerServiceConfig::SingleShard(
                ShardManagerServiceSingleShardConfig {},
            ),
        }
    }
}

impl SafeDisplay for DebugConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "{}",
            self.clone().into_golem_config().to_safe_string()
        );
        let _ = writeln!(&mut result, "cloud service:");
        let _ = writeln!(
            &mut result,
            "{}",
            self.cloud_service.to_safe_string_indented()
        );
        let _ = writeln!(&mut result, "CORS origin regex: {}", self.cors_origin_regex);
        result
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
            http_address: default_golem_config.http_address,
            http_port: default_golem_config.http_port,
            cloud_service: RemoteServiceConfig::default(),
            component_cache: ComponentCacheConfig::default(),
            component_service: ComponentServiceGrpcConfig::default(),
            project_service: ProjectServiceConfig::default(),
            agent_types_service: AgentTypesServiceConfig::default(),
            engine: EngineConfig::default(),
            resource_limits: ResourceLimitsConfig::default(),
            cors_origin_regex: "https://*.golem.cloud".to_string(),
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
