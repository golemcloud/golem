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

use super::*;

impl<Ctx: WorkerCtx> HasEvents for DirectWorkerInvocationRpc<Ctx> {
    fn events(&self) -> Arc<Events> {
        self.events.clone()
    }
}

impl<Ctx: WorkerCtx> HasActiveAgents<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn active_agents(&self) -> Arc<active_agents::ActiveAgents<Ctx>> {
        self.active_agents.clone()
    }
}

impl<Ctx: WorkerCtx> HasAgentTypesService for DirectWorkerInvocationRpc<Ctx> {
    fn agent_types(&self) -> Arc<dyn agent_types::AgentTypesService> {
        self.agent_types_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasAgentWebhooksService for DirectWorkerInvocationRpc<Ctx> {
    fn agent_webhooks(&self) -> Arc<AgentWebhooksService> {
        self.agent_webhooks_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasExternalDurableStreamService for DirectWorkerInvocationRpc<Ctx> {
    fn external_durable_streams(&self) -> Arc<dyn ExternalDurableStreamService> {
        self.external_durable_streams.clone()
    }
}

impl<Ctx: WorkerCtx> HasComponentService for DirectWorkerInvocationRpc<Ctx> {
    fn component_service(&self) -> Arc<dyn component::ComponentService> {
        self.component_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasCardService for DirectWorkerInvocationRpc<Ctx> {
    fn card_service(&self) -> Arc<dyn card::CardService> {
        self.card_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasConfig for DirectWorkerInvocationRpc<Ctx> {
    fn config(&self) -> Arc<golem_config::GolemConfig> {
        self.golem_config.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerService for DirectWorkerInvocationRpc<Ctx> {
    fn worker_service(&self) -> Arc<dyn worker::WorkerService> {
        self.worker_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerEnumerationService for DirectWorkerInvocationRpc<Ctx> {
    fn worker_enumeration_service(&self) -> Arc<dyn worker_enumeration::WorkerEnumerationService> {
        self.worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasRunningWorkerEnumerationService for DirectWorkerInvocationRpc<Ctx> {
    fn running_worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::RunningWorkerEnumerationService> {
        self.running_worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasPromiseService for DirectWorkerInvocationRpc<Ctx> {
    fn promise_service(&self) -> Arc<dyn promise::PromiseService> {
        self.promise_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWasmtimeEngine<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn engine(&self) -> Arc<wasmtime::Engine> {
        self.engine.clone()
    }

    fn linker(&self) -> Arc<wasmtime::component::Linker<Ctx>> {
        self.linker.clone()
    }

    fn runtime(&self) -> Handle {
        self.runtime.clone()
    }
}

impl<Ctx: WorkerCtx> HasKeyValueService for DirectWorkerInvocationRpc<Ctx> {
    fn key_value_service(&self) -> Arc<dyn key_value::KeyValueService> {
        self.key_value_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasBlobStoreService for DirectWorkerInvocationRpc<Ctx> {
    fn blob_store_service(&self) -> Arc<dyn blob_store::BlobStoreService> {
        self.blob_store_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasSchedulerService for DirectWorkerInvocationRpc<Ctx> {
    fn scheduler_service(&self) -> Arc<dyn scheduler::SchedulerService> {
        self.scheduler_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogService for DirectWorkerInvocationRpc<Ctx> {
    fn oplog_service(&self) -> Arc<dyn oplog::OplogService> {
        self.oplog_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerForkService for DirectWorkerInvocationRpc<Ctx> {
    fn worker_fork_service(&self) -> Arc<dyn worker_fork::WorkerForkService> {
        self.worker_fork.clone()
    }
}

impl<Ctx: WorkerCtx> HasRpc for DirectWorkerInvocationRpc<Ctx> {
    fn rpc(&self) -> Arc<dyn Rpc> {
        Arc::new(self.clone())
    }
}

impl<Ctx: WorkerCtx> HasLeakSentinel for DirectWorkerInvocationRpc<Ctx> {
    fn leak_sentinel(&self) -> Arc<()> {
        self.leak_sentinel.clone()
    }
}

impl<Ctx: WorkerCtx> HasExtraDeps<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn extra_deps(&self) -> Ctx::ExtraDeps {
        self.extra_deps.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardService for DirectWorkerInvocationRpc<Ctx> {
    fn shard_service(&self) -> Arc<dyn ShardService> {
        self.shard_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardManagerService for DirectWorkerInvocationRpc<Ctx> {
    fn shard_manager_service(&self) -> Arc<dyn shard_manager::ShardManagerService> {
        self.shard_manager_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasQuotaService for DirectWorkerInvocationRpc<Ctx> {
    fn quota_service(&self) -> Arc<dyn crate::services::quota::QuotaService> {
        self.quota_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerActivator<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn worker_activator(&self) -> Arc<dyn worker_activator::WorkerActivator<Ctx>> {
        self.worker_activator.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerProxy for DirectWorkerInvocationRpc<Ctx> {
    fn worker_proxy(&self) -> Arc<dyn WorkerProxy> {
        self.remote_rpc.worker_proxy.clone()
    }
}

impl<Ctx: WorkerCtx> HasFileLoader for DirectWorkerInvocationRpc<Ctx> {
    fn file_loader(&self) -> Arc<FileLoader> {
        self.file_loader.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogProcessorPlugin for DirectWorkerInvocationRpc<Ctx> {
    fn oplog_processor_plugin(&self) -> Arc<dyn OplogProcessorPlugin> {
        self.oplog_processor_plugin.clone()
    }
}

impl<Ctx: WorkerCtx> HasRdbmsService for DirectWorkerInvocationRpc<Ctx> {
    fn rdbms_service(&self) -> Arc<dyn rdbms::RdbmsService> {
        self.rdbms_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasResourceLimits for DirectWorkerInvocationRpc<Ctx> {
    fn resource_limits(&self) -> Arc<dyn ResourceLimits> {
        self.resource_limits.clone()
    }
}

impl<Ctx: WorkerCtx> HasShutdownToken for DirectWorkerInvocationRpc<Ctx> {
    fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
        self.shutdown_token.clone()
    }
}

impl<Ctx: WorkerCtx> HasHttpConnectionPool for DirectWorkerInvocationRpc<Ctx> {
    fn http_connection_pool(&self) -> Option<HttpConnectionPool> {
        self.http_connection_pool.clone()
    }
}

impl<Ctx: WorkerCtx> HasWebSocketConnectionPool for DirectWorkerInvocationRpc<Ctx> {
    fn websocket_connection_pool(&self) -> WebSocketConnectionPool {
        self.websocket_connection_pool.clone()
    }
}

impl<Ctx: WorkerCtx> HasAgentFilesystemSnapshots for DirectWorkerInvocationRpc<Ctx> {
    fn agent_filesystem_snapshots(&self) -> Arc<AgentFilesystemSnapshots> {
        self.agent_filesystem_snapshots.clone()
    }
}

impl<Ctx: WorkerCtx> HasMcpTransport for DirectWorkerInvocationRpc<Ctx> {
    fn mcp_transport(&self) -> Arc<crate::services::mcp::McpTransport> {
        self.mcp_transport.clone()
    }
}

impl<Ctx: WorkerCtx> HasEnvironmentStateService for DirectWorkerInvocationRpc<Ctx> {
    fn environment_state_service(&self) -> Arc<dyn EnvironmentStateService> {
        self.environment_state_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasNativeToolCatalog<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn native_tool_catalog(&self) -> Arc<crate::native_tool::NativeToolCatalog<Ctx>> {
        self.native_tool_catalog.clone()
    }
}
