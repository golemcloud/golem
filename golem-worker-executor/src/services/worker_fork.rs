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

pub(crate) mod admission;
pub(crate) mod export;
pub mod lineage;
mod payload;
pub(crate) mod publication;
pub mod stream_cut;

use super::agent_webhooks::AgentWebhooksService;
use super::environment_state::EnvironmentStateService;
use super::external_durable_stream::ExternalDurableStreamService;
use super::file_loader::FileLoader;
use super::{
    HasAgentWebhooksService, HasEnvironmentStateService, HasExternalDurableStreamService,
    HasMcpTransport,
};
use crate::durable_host::durable_stream::{DurableStreamStore, StreamStoreError};
use crate::durable_host::websocket::WebSocketConnectionPool;
use crate::metrics::workers::record_worker_call;
use crate::model::ExecutionStatus;
use crate::services::events::Events;
use crate::services::oplog::plugin::OplogProcessorPlugin;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps, OplogServiceOps};
use crate::services::resource_limits::ResourceLimits;
use crate::services::rpc::Rpc;
use crate::services::shard::ShardService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{
    HasActiveAgents, HasAgentTypesService, HasBlobStoreService, HasCardService,
    HasComponentService, HasConfig, HasEvents, HasExtraDeps, HasFileLoader, HasHttpConnectionPool,
    HasKeyValueService, HasLeakSentinel, HasNativeToolCatalog, HasOplogProcessorPlugin,
    HasOplogService, HasPromiseService, HasQuotaService, HasResourceLimits, HasRpc,
    HasRunningWorkerEnumerationService, HasSchedulerService, HasShardManagerService,
    HasShardService, HasShutdownToken, HasWasmtimeEngine, HasWebSocketConnectionPool,
    HasWorkerActivator, HasWorkerEnumerationService, HasWorkerProxy, HasWorkerService,
    active_agents, agent_types, blob_store, card, component, golem_config, key_value, oplog,
    promise, scheduler, shard_manager, worker, worker_activator, worker_enumeration,
};
use crate::services::{HasRdbmsService, HasWorkerForkService, rdbms};
use crate::worker::status::calculate_last_known_status_with_checkpoint;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ForkStreamSlotRequest, ForkStreamSlotResponse,
};
use golem_common::base_model::component::ComponentRevision;
use golem_common::base_model::oplog::QueuedCardEvent;
use golem_common::base_model::regions::DeletedRegionsBuilder;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentMode, OwnerKind};
use golem_common::model::card::{AgentCardHolder, CardHolder};
use golem_common::model::durable_stream::StreamSessionRecord;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogIndexRange};
use golem_common::model::{AgentFingerprint, AgentMetadata, Timestamp};
use golem_common::model::{AgentId, IdempotencyKey, OwnedAgentId};
use golem_common::read_only_lock;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::runtime::Handle;
use uuid::Uuid;
use wasmtime_wasi_http::HttpConnectionPool;

#[async_trait]
pub trait WorkerForkService: Send + Sync {
    async fn fork_stream_slot(&self, request: ForkStreamSlotRequest) -> ForkStreamSlotResponse;
    // TODO: this should be restricted to targets within the same component
    async fn fork(
        &self,
        fork_account_id: AccountId,
        source_agent_id: &OwnedAgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerExecutorError>;

    // TODO: this should be restricted to targets within the same component
    async fn fork_and_write_fork_result(
        &self,
        fork_account_id: AccountId,
        source_agent_id: &OwnedAgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        copied_scope_start: Option<OplogIndex>,
        forked_phantom_id: Uuid,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerExecutorError>;
}

pub struct DefaultWorkerFork<Ctx: WorkerCtx> {
    pub rpc: Arc<dyn Rpc>,
    pub active_agents: Arc<active_agents::ActiveAgents<Ctx>>,
    pub agent_types: Arc<dyn agent_types::AgentTypesService>,
    pub agent_webhooks: Arc<AgentWebhooksService>,
    pub external_durable_streams: Arc<dyn ExternalDurableStreamService>,
    pub engine: Arc<wasmtime::Engine>,
    pub linker: Arc<wasmtime::component::Linker<Ctx>>,
    pub runtime: Handle,
    pub card_service: Arc<dyn card::CardService>,
    pub component_service: Arc<dyn component::ComponentService>,
    pub shard_manager_service: Arc<dyn shard_manager::ShardManagerService>,
    pub quota_service: Arc<dyn crate::services::quota::QuotaService>,
    pub worker_service: Arc<dyn worker::WorkerService>,
    pub worker_proxy: Arc<dyn WorkerProxy>,
    pub worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
    pub running_worker_enumeration_service:
        Arc<dyn worker_enumeration::RunningWorkerEnumerationService>,
    pub promise_service: Arc<dyn promise::PromiseService>,
    pub golem_config: Arc<golem_config::GolemConfig>,
    pub shard_service: Arc<dyn ShardService>,
    pub key_value_service: Arc<dyn key_value::KeyValueService>,
    pub blob_store_service: Arc<dyn blob_store::BlobStoreService>,
    pub rdbms_service: Arc<dyn rdbms::RdbmsService>,
    pub oplog_service: Arc<dyn oplog::OplogService>,
    pub scheduler_service: Arc<dyn scheduler::SchedulerService>,
    pub worker_activator: Arc<dyn worker_activator::WorkerActivator<Ctx>>,
    pub events: Arc<Events>,
    pub file_loader: Arc<FileLoader>,
    pub oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
    pub resource_limits: Arc<dyn ResourceLimits>,
    pub shutdown_token: tokio_util::sync::CancellationToken,
    pub http_connection_pool: Option<HttpConnectionPool>,
    pub websocket_connection_pool: WebSocketConnectionPool,
    pub mcp_transport: Arc<super::mcp::McpTransport>,
    pub environment_state_service: Arc<dyn EnvironmentStateService>,
    pub native_tool_catalog: Arc<crate::native_tool::NativeToolCatalog<Ctx>>,
    pub extra_deps: Ctx::ExtraDeps,
    pub leak_sentinel: Arc<()>,
}

impl<Ctx: WorkerCtx> HasEvents for DefaultWorkerFork<Ctx> {
    fn events(&self) -> Arc<Events> {
        self.events.clone()
    }
}

impl<Ctx: WorkerCtx> HasActiveAgents<Ctx> for DefaultWorkerFork<Ctx> {
    fn active_agents(&self) -> Arc<active_agents::ActiveAgents<Ctx>> {
        self.active_agents.clone()
    }
}

impl<Ctx: WorkerCtx> HasAgentTypesService for DefaultWorkerFork<Ctx> {
    fn agent_types(&self) -> Arc<dyn agent_types::AgentTypesService> {
        self.agent_types.clone()
    }
}

impl<Ctx: WorkerCtx> HasAgentWebhooksService for DefaultWorkerFork<Ctx> {
    fn agent_webhooks(&self) -> Arc<AgentWebhooksService> {
        self.agent_webhooks.clone()
    }
}

impl<Ctx: WorkerCtx> HasExternalDurableStreamService for DefaultWorkerFork<Ctx> {
    fn external_durable_streams(&self) -> Arc<dyn ExternalDurableStreamService> {
        self.external_durable_streams.clone()
    }
}

impl<Ctx: WorkerCtx> HasComponentService for DefaultWorkerFork<Ctx> {
    fn component_service(&self) -> Arc<dyn component::ComponentService> {
        self.component_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasCardService for DefaultWorkerFork<Ctx> {
    fn card_service(&self) -> Arc<dyn card::CardService> {
        self.card_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasConfig for DefaultWorkerFork<Ctx> {
    fn config(&self) -> Arc<golem_config::GolemConfig> {
        self.golem_config.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerService for DefaultWorkerFork<Ctx> {
    fn worker_service(&self) -> Arc<dyn worker::WorkerService> {
        self.worker_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerEnumerationService for DefaultWorkerFork<Ctx> {
    fn worker_enumeration_service(&self) -> Arc<dyn worker_enumeration::WorkerEnumerationService> {
        self.worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasRunningWorkerEnumerationService for DefaultWorkerFork<Ctx> {
    fn running_worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::RunningWorkerEnumerationService> {
        self.running_worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasPromiseService for DefaultWorkerFork<Ctx> {
    fn promise_service(&self) -> Arc<dyn promise::PromiseService> {
        self.promise_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWasmtimeEngine<Ctx> for DefaultWorkerFork<Ctx> {
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

impl<Ctx: WorkerCtx> HasKeyValueService for DefaultWorkerFork<Ctx> {
    fn key_value_service(&self) -> Arc<dyn key_value::KeyValueService> {
        self.key_value_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasRdbmsService for DefaultWorkerFork<Ctx> {
    fn rdbms_service(&self) -> Arc<dyn rdbms::RdbmsService> {
        self.rdbms_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasBlobStoreService for DefaultWorkerFork<Ctx> {
    fn blob_store_service(&self) -> Arc<dyn blob_store::BlobStoreService> {
        self.blob_store_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasSchedulerService for DefaultWorkerFork<Ctx> {
    fn scheduler_service(&self) -> Arc<dyn scheduler::SchedulerService> {
        self.scheduler_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogService for DefaultWorkerFork<Ctx> {
    fn oplog_service(&self) -> Arc<dyn oplog::OplogService> {
        self.oplog_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerForkService for DefaultWorkerFork<Ctx> {
    fn worker_fork_service(&self) -> Arc<dyn WorkerForkService> {
        Arc::new(self.clone())
    }
}

impl<Ctx: WorkerCtx> HasRpc for DefaultWorkerFork<Ctx> {
    fn rpc(&self) -> Arc<dyn Rpc> {
        self.rpc.clone()
    }
}

impl<Ctx: WorkerCtx> HasExtraDeps<Ctx> for DefaultWorkerFork<Ctx> {
    fn extra_deps(&self) -> Ctx::ExtraDeps {
        self.extra_deps.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardService for DefaultWorkerFork<Ctx> {
    fn shard_service(&self) -> Arc<dyn ShardService> {
        self.shard_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardManagerService for DefaultWorkerFork<Ctx> {
    fn shard_manager_service(&self) -> Arc<dyn shard_manager::ShardManagerService> {
        self.shard_manager_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasQuotaService for DefaultWorkerFork<Ctx> {
    fn quota_service(&self) -> Arc<dyn crate::services::quota::QuotaService> {
        self.quota_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerActivator<Ctx> for DefaultWorkerFork<Ctx> {
    fn worker_activator(&self) -> Arc<dyn worker_activator::WorkerActivator<Ctx>> {
        self.worker_activator.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerProxy for DefaultWorkerFork<Ctx> {
    fn worker_proxy(&self) -> Arc<dyn WorkerProxy> {
        self.worker_proxy.clone()
    }
}

impl<Ctx: WorkerCtx> HasFileLoader for DefaultWorkerFork<Ctx> {
    fn file_loader(&self) -> Arc<FileLoader> {
        self.file_loader.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogProcessorPlugin for DefaultWorkerFork<Ctx> {
    fn oplog_processor_plugin(&self) -> Arc<dyn OplogProcessorPlugin> {
        self.oplog_processor_plugin.clone()
    }
}

impl<Ctx: WorkerCtx> HasResourceLimits for DefaultWorkerFork<Ctx> {
    fn resource_limits(&self) -> Arc<dyn ResourceLimits> {
        self.resource_limits.clone()
    }
}

impl<Ctx: WorkerCtx> HasShutdownToken for DefaultWorkerFork<Ctx> {
    fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
        self.shutdown_token.clone()
    }
}

impl<Ctx: WorkerCtx> HasLeakSentinel for DefaultWorkerFork<Ctx> {
    fn leak_sentinel(&self) -> Arc<()> {
        self.leak_sentinel.clone()
    }
}

impl<Ctx: WorkerCtx> HasHttpConnectionPool for DefaultWorkerFork<Ctx> {
    fn http_connection_pool(&self) -> Option<HttpConnectionPool> {
        self.http_connection_pool.clone()
    }
}

impl<Ctx: WorkerCtx> HasWebSocketConnectionPool for DefaultWorkerFork<Ctx> {
    fn websocket_connection_pool(&self) -> WebSocketConnectionPool {
        self.websocket_connection_pool.clone()
    }
}

impl<Ctx: WorkerCtx> HasMcpTransport for DefaultWorkerFork<Ctx> {
    fn mcp_transport(&self) -> Arc<super::mcp::McpTransport> {
        self.mcp_transport.clone()
    }
}

impl<Ctx: WorkerCtx> HasEnvironmentStateService for DefaultWorkerFork<Ctx> {
    fn environment_state_service(&self) -> Arc<dyn EnvironmentStateService> {
        self.environment_state_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasNativeToolCatalog<Ctx> for DefaultWorkerFork<Ctx> {
    fn native_tool_catalog(&self) -> Arc<crate::native_tool::NativeToolCatalog<Ctx>> {
        self.native_tool_catalog.clone()
    }
}

impl<Ctx: WorkerCtx> Clone for DefaultWorkerFork<Ctx> {
    fn clone(&self) -> Self {
        Self {
            rpc: self.rpc.clone(),
            active_agents: self.active_agents.clone(),
            agent_types: self.agent_types.clone(),
            agent_webhooks: self.agent_webhooks.clone(),
            external_durable_streams: self.external_durable_streams.clone(),
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            runtime: self.runtime.clone(),
            card_service: self.card_service.clone(),
            component_service: self.component_service.clone(),
            shard_manager_service: self.shard_manager_service.clone(),
            quota_service: self.quota_service.clone(),
            worker_service: self.worker_service.clone(),
            worker_proxy: self.worker_proxy.clone(),
            worker_enumeration_service: self.worker_enumeration_service.clone(),
            running_worker_enumeration_service: self.running_worker_enumeration_service.clone(),
            promise_service: self.promise_service.clone(),
            golem_config: self.golem_config.clone(),
            shard_service: self.shard_service.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
            rdbms_service: self.rdbms_service.clone(),
            oplog_service: self.oplog_service.clone(),
            scheduler_service: self.scheduler_service.clone(),
            worker_activator: self.worker_activator.clone(),
            events: self.events.clone(),
            file_loader: self.file_loader.clone(),
            oplog_processor_plugin: self.oplog_processor_plugin.clone(),
            resource_limits: self.resource_limits.clone(),
            shutdown_token: self.shutdown_token.clone(),
            http_connection_pool: self.http_connection_pool.clone(),
            websocket_connection_pool: self.websocket_connection_pool.clone(),
            environment_state_service: self.environment_state_service.clone(),
            native_tool_catalog: self.native_tool_catalog.clone(),
            mcp_transport: self.mcp_transport.clone(),
            extra_deps: self.extra_deps.clone(),
            leak_sentinel: self.leak_sentinel.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> DefaultWorkerFork<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rpc: Arc<dyn Rpc>,
        active_agents: Arc<active_agents::ActiveAgents<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        card_service: Arc<dyn card::CardService>,
        component_service: Arc<dyn component::ComponentService>,
        shard_manager_service: Arc<dyn shard_manager::ShardManagerService>,
        quota_service: Arc<dyn crate::services::quota::QuotaService>,
        worker_service: Arc<dyn worker::WorkerService>,
        worker_proxy: Arc<dyn WorkerProxy>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService,
        >,
        promise_service: Arc<dyn promise::PromiseService>,
        golem_config: Arc<golem_config::GolemConfig>,
        shard_service: Arc<dyn ShardService>,
        key_value_service: Arc<dyn key_value::KeyValueService>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService>,
        rdbms_service: Arc<dyn rdbms::RdbmsService>,
        oplog_service: Arc<dyn oplog::OplogService>,
        scheduler_service: Arc<dyn scheduler::SchedulerService>,
        worker_activator: Arc<dyn worker_activator::WorkerActivator<Ctx>>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin>,
        resource_limits: Arc<dyn ResourceLimits>,
        environment_state_service: Arc<dyn EnvironmentStateService>,
        native_tool_catalog: Arc<crate::native_tool::NativeToolCatalog<Ctx>>,
        agent_types: Arc<dyn agent_types::AgentTypesService>,
        agent_webhooks: Arc<AgentWebhooksService>,
        external_durable_streams: Arc<dyn ExternalDurableStreamService>,
        shutdown_token: tokio_util::sync::CancellationToken,
        http_connection_pool: Option<HttpConnectionPool>,
        websocket_connection_pool: WebSocketConnectionPool,
        mcp_transport: Arc<super::mcp::McpTransport>,
        extra_deps: Ctx::ExtraDeps,
        leak_sentinel: Arc<()>,
    ) -> Self {
        Self {
            rpc,
            active_agents,
            agent_types,
            agent_webhooks,
            external_durable_streams,
            engine,
            linker,
            runtime,
            card_service,
            component_service,
            shard_manager_service,
            quota_service,
            worker_service,
            worker_proxy,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config,
            shard_service,
            key_value_service,
            blob_store_service,
            rdbms_service,
            oplog_service,
            scheduler_service,
            worker_activator,
            events,
            file_loader,
            oplog_processor_plugin,
            resource_limits,
            shutdown_token,
            http_connection_pool,
            websocket_connection_pool,
            mcp_transport,
            environment_state_service,
            native_tool_catalog,
            extra_deps,
            leak_sentinel,
        }
    }

    async fn validate_worker_forking(
        &self,
        environment_id: EnvironmentId,
        source_agent_id: &AgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<(OwnedAgentId, OwnedAgentId), WorkerExecutorError> {
        OwnerKind::ComponentAgent
            .validate_instance_name(&target_agent_id.agent_id)
            .map_err(WorkerExecutorError::invalid_request)?;
        let second_index = OplogIndex::INITIAL.next();

        if oplog_index_cut_off < second_index {
            return Err(WorkerExecutorError::invalid_request(
                "oplog_index_cut_off must be at least 2",
            ));
        }

        let owned_target_agent_id = OwnedAgentId::new(environment_id, target_agent_id);

        if source_agent_id == target_agent_id {
            return Err(WorkerExecutorError::worker_already_exists(
                target_agent_id.clone(),
            ));
        }
        if source_agent_id.component_id != target_agent_id.component_id {
            return Err(WorkerExecutorError::invalid_request(
                "Source and target must belong to the same component",
            ));
        }

        // ADMISSION: this is the only ownership check on the
        // fork path and it rejects rather than routing, so a fork started after
        // the lease lapsed must be refused.
        self.shard_service.check_admission(source_agent_id)?;

        let owned_source_agent_id = OwnedAgentId::new(environment_id, source_agent_id);

        let source_metadata = self
            .worker_service
            .get(&owned_source_agent_id)
            .await?
            .ok_or(WorkerExecutorError::worker_not_found(
                source_agent_id.clone(),
            ))?;
        if source_metadata.initial_worker_metadata.owner_kind == OwnerKind::EphemeralExternalTool {
            return Err(WorkerExecutorError::invalid_request(
                "External-tool owners cannot be forked",
            ));
        }

        Ok((owned_source_agent_id, owned_target_agent_id))
    }

    async fn copy_source_oplog(
        &self,
        fork_account_id: AccountId,
        source_agent_id: &OwnedAgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        source_fingerprint: AgentFingerprint,
        stage_id: Uuid,
        request_hash: [u8; 32],
        selected: Option<(
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffset>,
        )>,
        max_copied_bytes: Option<u64>,
        export: Option<&export::Candidate>,
    ) -> Result<(Arc<dyn Oplog>, u64, AgentFingerprint), WorkerExecutorError> {
        record_worker_call("fork");

        tracing::debug!(
            "Copying source oplog of worker {fork_account_id}/{source_agent_id} to {target_agent_id} up to index {oplog_index_cut_off}"
        );

        let mut source_lifecycle = self
            .oplog_service
            .lock_lifecycle(&source_agent_id.agent_id)
            .await;
        let (owned_source_agent_id, owned_target_agent_id) = self
            .validate_worker_forking(
                source_agent_id.environment_id,
                &source_agent_id.agent_id,
                target_agent_id,
                oplog_index_cut_off,
            )
            .await?;

        let target_agent_id = owned_target_agent_id.agent_id.clone();
        let environment_id = owned_target_agent_id.environment_id;

        let source = self
            .worker_service
            .get(&owned_source_agent_id)
            .await?
            .ok_or_else(|| {
                WorkerExecutorError::worker_not_found(owned_source_agent_id.agent_id())
            })?;
        let initial_source_worker_metadata = source.initial_worker_metadata;
        if initial_source_worker_metadata.fingerprint != source_fingerprint {
            return Err(WorkerExecutorError::invalid_request(
                "Fork source was recreated",
            ));
        }
        let agent_mode = initial_source_worker_metadata.agent_mode;
        if agent_mode != AgentMode::Durable {
            return Err(WorkerExecutorError::invalid_request(
                "Only durable agents can be forked",
            ));
        }
        let source_status = calculate_last_known_status_with_checkpoint(
            self,
            &owned_source_agent_id,
            initial_source_worker_metadata.fingerprint,
            agent_mode,
            source.last_known_status,
        )
        .await
        .map_err(WorkerExecutorError::runtime)?
        .ok_or_else(|| WorkerExecutorError::worker_not_found(owned_source_agent_id.agent_id()))?;

        // The stage id also identifies the target incarnation while it is hidden. This lets
        // scheduled work for that incarnation distinguish publication in progress from a target
        // that genuinely does not exist.
        let instance_id = stage_id;
        let source_oplog_metadata = initial_source_worker_metadata.clone();

        // Use the source worker's `created_by` (the component owner) rather
        // than `fork_account_id` (the fork caller). This ensures the forked
        // worker's metadata is consistent with its initial oplog entry (which
        // preserves the source's `created_by`), and that resource consumption
        // is attributed to the component owner, not the caller.
        // See https://github.com/golemcloud/golem/issues/3099
        let target_worker_metadata = AgentMetadata {
            agent_id: target_agent_id.clone(),
            owner_kind: initial_source_worker_metadata.owner_kind,
            created_by: initial_source_worker_metadata.created_by,
            created_by_email: initial_source_worker_metadata.created_by_email,
            environment_id,
            env: initial_source_worker_metadata.env,
            config: initial_source_worker_metadata.config,
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: initial_source_worker_metadata.last_known_status.clone(),
            original_phantom_id: initial_source_worker_metadata.original_phantom_id,
            fingerprint: AgentFingerprint(instance_id),
            agent_mode,
        };

        let source_oplog = self
            .oplog_service
            .open(
                &mut source_lifecycle,
                &owned_source_agent_id,
                agent_mode,
                None,
                source_oplog_metadata,
                read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(
                    arc_swap::ArcSwap::from_pointee(source_status),
                )),
                read_only_lock::std::ReadOnlyLock::new(Arc::new(std::sync::RwLock::new(
                    ExecutionStatus::Suspended {
                        agent_mode,
                        timestamp: Timestamp::now_utc(),
                    },
                ))),
            )
            .await;
        let source_oplog = Ctx::wrap_oplog(
            owned_source_agent_id.clone(),
            source_oplog,
            self.extra_deps.clone(),
        );
        let read_source = |index| {
            let source = &owned_source_agent_id;
            async move {
                self.oplog_service
                    .read_exact(source, agent_mode, index, 1)
                    .await
                    .remove(&index)
                    .expect("fork source oplog entry is missing")
            }
        };

        // Copy the inclusive prefix. Ordinary calls recover from that prefix, even if their
        // terminal or delivery marker is absent. Atomic and transaction outcomes remain paired.
        let committed_end = self
            .oplog_service
            .get_last_index(&owned_source_agent_id, agent_mode)
            .await;
        // Debug playback exposes only its target prefix, even when storage contains later entries.
        let readable_end = committed_end.min(source_oplog.current_oplog_index().await);
        let source_oplog_end = export.map_or(readable_end, |candidate| candidate.horizon);
        if source_oplog_end > readable_end {
            return Err(WorkerExecutorError::invalid_request(
                "Fork source horizon is unavailable",
            ));
        }
        if oplog_index_cut_off > source_oplog_end {
            return Err(WorkerExecutorError::invalid_request(
                "Fork cut exceeds committed source history",
            ));
        }
        let source_skipped_regions = crate::worker::status::skipped_regions_at(
            self,
            &owned_source_agent_id,
            source_oplog_end,
        )
        .await
        .map_err(WorkerExecutorError::runtime)?;
        if let Some(spanning) = crate::worker::cut_point::find_construct_spanning_cut_point(
            read_source,
            oplog_index_cut_off,
            source_oplog_end,
            &source_skipped_regions,
        )
        .await
        {
            return Err(WorkerExecutorError::invalid_request(format!(
                "Cannot fork worker at oplog index {oplog_index_cut_off}: the cut point is inside {spanning}"
            )));
        }

        let mut fork_cut = DurableStreamStore::prepare_fork_cut(
            source_oplog.as_ref(),
            (&owned_source_agent_id, source_fingerprint),
            (&owned_target_agent_id, AgentFingerprint(instance_id)),
            source_oplog_end,
            oplog_index_cut_off,
            selected,
            request_hash,
            false,
        )
        .await
        .map_err(|error| match error {
            StreamStoreError::InvalidOffset(_) | StreamStoreError::UnknownStream(_) => {
                WorkerExecutorError::invalid_request(error.to_string())
            }
            _ => WorkerExecutorError::runtime(error.to_string()),
        })?;
        fork_cut.export = export.map(|candidate| candidate.export.clone());

        let initial_oplog_entry = read_source(OplogIndex::INITIAL).await;
        let initial_size = golem_common::serialization::serialize(&initial_oplog_entry)
            .map_err(WorkerExecutorError::runtime)?
            .len() as u64;

        // Update the oplog initial entry with the new worker
        let target_initial_oplog_entry =
            Self::update_agent_id(initial_oplog_entry, &target_agent_id, instance_id).ok_or(
                WorkerExecutorError::unknown("Failed to update worker id in oplog entry"),
            )?;

        let new_oplog = self
            .oplog_service
            .create_staged(
                &owned_target_agent_id,
                agent_mode,
                stage_id,
                target_worker_metadata,
            )
            .await
            .map_err(WorkerExecutorError::runtime)?;
        new_oplog.add(target_initial_oplog_entry).await;

        let oplog_range = OplogIndexRange::new(OplogIndex::INITIAL.next(), oplog_index_cut_off);

        // Track unmatched work so the fork can cancel unrelated queued invocations and
        // updates. Export forks retain their selected invocation and its constructor.
        let mut pending_invocation_keys: Vec<(IdempotencyKey, OplogIndex)> = Vec::new();
        let mut pending_update_revisions: Vec<ComponentRevision> = Vec::new();
        let mut deleted_regions_builder = DeletedRegionsBuilder::new();
        let mut copied_bytes = initial_size;
        let external_payload_bytes = Arc::new(AtomicU64::new(0));

        for oplog_index in oplog_range {
            let mut entry = rewrite_forked_oplog_entry(
                read_source(oplog_index).await,
                &owned_source_agent_id.agent_id,
                &owned_target_agent_id.agent_id,
            );
            copied_bytes = copied_bytes.saturating_add(
                golem_common::serialization::serialize(&entry)
                    .map_err(WorkerExecutorError::runtime)?
                    .len() as u64,
            );
            payload::copy_entry_payloads(&mut entry, |payload_id, md5_hash| {
                let new_oplog = &new_oplog;
                let source = &owned_source_agent_id;
                let external_payload_bytes = external_payload_bytes.clone();
                async move {
                    let bytes = self
                        .oplog_service
                        .download_raw_payload(source, agent_mode, payload_id, md5_hash)
                        .await?;
                    external_payload_bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                    new_oplog.upload_raw_payload(bytes).await
                }
            })
            .await
            .map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "Failed copying fork payload at oplog index {oplog_index}: {error}"
                ))
            })?;
            let counted_bytes =
                copied_bytes.saturating_add(external_payload_bytes.load(Ordering::Relaxed));
            if max_copied_bytes.is_some_and(|limit| counted_bytes > limit) {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "fork copied bytes {counted_bytes} exceed limit"
                )));
            }
            // The append-time session fold needs a decoded value even when its durable bytes
            // remain external. Read the new target reference, never trust the source's cache.
            if let OplogEntry::StreamSession { record, .. } = &mut entry
                && matches!(
                    record,
                    golem_common::model::oplog::OplogPayload::External { cached: None, .. }
                )
            {
                let value = new_oplog
                    .download_payload(record.clone())
                    .await
                    .map_err(|error| {
                        WorkerExecutorError::runtime(format!(
                            "Failed hydrating fork session at oplog index {oplog_index}: {error}"
                        ))
                    })?;
                if let golem_common::model::oplog::OplogPayload::External { cached, .. } = record {
                    *cached = Some(Arc::new(value));
                }
            }
            new_oplog.add(entry.clone()).await;

            if let OplogEntry::Revert { dropped_region, .. } = &entry {
                deleted_regions_builder.add(dropped_region.clone());
            }

            // For pending invocations, deleted regions don't matter - both inputs
            // and outputs in deleted regions are still accounted for (see calculate_pending_invocations).
            match &entry {
                OplogEntry::PendingAgentInvocation {
                    idempotency_key, ..
                } => {
                    pending_invocation_keys.push((idempotency_key.clone(), oplog_index));
                }
                OplogEntry::AgentInvocationStarted {
                    idempotency_key, ..
                } => {
                    pending_invocation_keys.retain(|(key, _)| key != idempotency_key);
                }
                OplogEntry::CancelPendingInvocation {
                    idempotency_key, ..
                } => {
                    pending_invocation_keys.retain(|(key, _)| key != idempotency_key);
                }
                _ => {}
            }
        }

        // For pending updates, we need to respect deleted regions (see calculate_update_fields).
        let deleted_regions = deleted_regions_builder.build();
        let oplog_range = OplogIndexRange::new(OplogIndex::INITIAL.next(), oplog_index_cut_off);
        for oplog_index in oplog_range {
            if deleted_regions.is_in_deleted_region(oplog_index) {
                continue;
            }
            let entry = read_source(oplog_index).await;
            match &entry {
                OplogEntry::PendingUpdate { description, .. } => {
                    pending_update_revisions.push(*description.target_revision());
                }
                OplogEntry::SuccessfulUpdate { .. } | OplogEntry::FailedUpdate { .. }
                    // Pop front to match calculate_update_fields semantics
                    if !pending_update_revisions.is_empty() => {
                        pending_update_revisions.remove(0);
                    }
                _ => {}
            }
        }

        // The marker precedes every target-authored cancellation or synthetic result.
        let now = Timestamp::now_utc();
        let record = new_oplog
            .upload_payload(&StreamSessionRecord::ForkCut(fork_cut.clone()))
            .await
            .map_err(WorkerExecutorError::runtime)?;
        new_oplog
            .add(OplogEntry::StreamSession {
                timestamp: now,
                entity_parent_start_index: None,
                record,
            })
            .await;

        for (idempotency_key, pending_index) in pending_invocation_keys {
            if let Some(candidate) = export {
                if idempotency_key == candidate.source_invocation.idempotency_key {
                    continue;
                }
                if let OplogEntry::PendingAgentInvocation { payload, .. } =
                    read_source(pending_index).await
                    && matches!(
                        self.oplog_service
                            .download_payload(&owned_source_agent_id, agent_mode, payload)
                            .await
                            .map_err(WorkerExecutorError::runtime)?,
                        golem_common::model::AgentInvocationPayload::AgentInitialization { .. }
                    )
                {
                    continue;
                }
            }
            tracing::debug!("Cancelling pending invocation {idempotency_key} in forked worker");
            new_oplog
                .add(OplogEntry::CancelPendingInvocation {
                    timestamp: now,
                    idempotency_key,
                })
                .await;
        }

        for target_revision in pending_update_revisions {
            tracing::debug!(
                "Cancelling pending update to revision {target_revision} in forked worker"
            );
            new_oplog
                .add(OplogEntry::FailedUpdate {
                    timestamp: now,
                    target_revision,
                    details: Some("cancelled by fork".to_string()),
                })
                .await;
        }

        if let Some(candidate) = export
            && (candidate.initial.is_some() || candidate.export.closed)
        {
            let selected = fork_cut.selected_stream_id.ok_or_else(|| {
                WorkerExecutorError::runtime("Fork selected input registration is missing")
            })?;
            let producer = DurableStreamStore::load(
                new_oplog.clone(),
                environment_id,
                target_agent_id.clone(),
                AgentFingerprint(instance_id),
                None,
            )
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            let mapping = producer
                .materialize_binding(&golem_common::model::durable_stream::StreamBindingRecord {
                    transport_stream_id: 0,
                    source: golem_common::model::durable_stream::StreamRecordReference::Local(
                        selected,
                    ),
                    role: golem_common::model::durable_stream::SessionStreamRole::Input,
                })
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            let result = producer
                .append_external_input(
                    None,
                    &mapping.handle.source_invocation,
                    mapping.handle.stream_id,
                    candidate.initial.clone(),
                    candidate.export.closed,
                    None,
                )
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            if !matches!(
                result,
                crate::durable_host::durable_stream::ExternalAppendOutcome::Accepted(_)
            ) {
                return Err(WorkerExecutorError::runtime(
                    "Fork initial input was not accepted",
                ));
            }
        }
        drop(source_oplog);
        drop(source_lifecycle);
        Ok((
            new_oplog,
            copied_bytes.saturating_add(external_payload_bytes.load(Ordering::Relaxed)),
            AgentFingerprint(instance_id),
        ))
    }

    async fn perform_fork(
        &self,
        fork_account_id: AccountId,
        source: &OwnedAgentId,
        target_agent_id: &AgentId,
        cut: OplogIndex,
        guest_result: Option<(Option<OplogIndex>, Uuid)>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerExecutorError> {
        self.validate_worker_forking(
            source.environment_id,
            &source.agent_id,
            target_agent_id,
            cut,
        )
        .await?;
        let source_metadata = self
            .worker_service
            .get(source)
            .await?
            .ok_or_else(|| WorkerExecutorError::worker_not_found(source.agent_id.clone()))?
            .initial_worker_metadata;
        let hash =
            publication::request_hash(source, source_metadata.fingerprint, cut, None, guest_result)
                .map_err(WorkerExecutorError::runtime)?;
        let target = OwnedAgentId::new(source.environment_id, target_agent_id);
        if !publication::existing_fork(self.oplog_service.as_ref(), &target, cut, hash).await? {
            let stage_id = Uuid::new_v4();
            let result = async {
                let (oplog, _, _) = self
                    .copy_source_oplog(
                        fork_account_id,
                        source,
                        target_agent_id,
                        cut,
                        source_metadata.fingerprint,
                        stage_id,
                        hash,
                        None,
                        None,
                        None,
                    )
                    .await?;
                if let Some((scope, phantom)) = guest_result {
                    publication::write_guest_result(oplog.as_ref(), scope, phantom).await?;
                }
                oplog.commit(CommitLevel::Always).await;
                let last = oplog.current_oplog_index().await;
                drop(oplog);
                let target_lifecycle = self.oplog_service.lock_lifecycle(&target.agent_id).await;
                let publication = self
                    .oplog_service
                    .publish_staged(&target, AgentMode::Durable, stage_id, last)
                    .await;
                let result = match publication {
                    Ok(true) => Ok(()),
                    outcome => {
                        if publication::existing_fork(
                            self.oplog_service.as_ref(),
                            &target,
                            cut,
                            hash,
                        )
                        .await?
                        {
                            Ok(())
                        } else {
                            Err(WorkerExecutorError::runtime(outcome.err().unwrap_or_else(
                                || "Fork publication lost its target before reconciliation".into(),
                            )))
                        }
                    }
                };
                drop(target_lifecycle);
                result
            }
            .await;
            // Each attempt has its own writer. Cleanup must never remove the winner's payloads.
            let cleanup = self
                .oplog_service
                .discard_staged(&target, AgentMode::Durable, stage_id)
                .await;
            if let Err(error) = cleanup {
                tracing::warn!(
                    agent_id = %target,
                    stage_id = %stage_id,
                    error = %error,
                    "Failed to discard hidden fork stage"
                );
            }
            result?;
        }
        // Resume can fail after publication, so retries reconcile the immutable marker and
        // retry this operation without replacing the fork or appending a second synthetic result.
        self.worker_proxy
            .resume(target_agent_id, true, auth_ctx)
            .await
            .map_err(|error| {
                WorkerExecutorError::failed_to_resume_worker(target_agent_id.clone(), error.into())
            })
    }

    pub fn update_agent_id(
        entry: OplogEntry,
        agent_id: &AgentId,
        instance_id: Uuid,
    ) -> Option<OplogEntry> {
        match entry {
            OplogEntry::Create {
                timestamp,
                mut parameters,
            } => {
                parameters.agent_id = agent_id.clone();
                parameters.instance_id = instance_id;
                Some(OplogEntry::Create {
                    timestamp,
                    parameters,
                })
            }
            _ => None,
        }
    }
}

fn rewrite_forked_agent_holder(
    holder: &mut CardHolder,
    source_agent_id: &AgentId,
    target_agent_id: &AgentId,
) {
    if matches!(holder, CardHolder::Agent(holder) if holder.agent_id == *source_agent_id) {
        *holder = CardHolder::Agent(AgentCardHolder {
            agent_id: target_agent_id.clone(),
        });
    }
}

fn rewrite_forked_oplog_entry(
    mut entry: OplogEntry,
    source_agent_id: &AgentId,
    target_agent_id: &AgentId,
) -> OplogEntry {
    match &mut entry {
        OplogEntry::AgentInvocationStarted { wallet_pin, .. } => {
            wallet_pin.wallet_token.wallet_id_hash = CardHolder::Agent(AgentCardHolder {
                agent_id: target_agent_id.clone(),
            })
            .wallet_id_hash();
        }
        OplogEntry::CardEventQueued { event, .. } => {
            if let QueuedCardEvent::TransferStarted(event) = event.as_mut() {
                rewrite_forked_agent_holder(
                    &mut event.target_holder,
                    source_agent_id,
                    target_agent_id,
                );
            }
        }
        OplogEntry::CardTransferStarted {
            source_holder,
            target_holder,
            ..
        } => {
            rewrite_forked_agent_holder(source_holder, source_agent_id, target_agent_id);
            rewrite_forked_agent_holder(target_holder, source_agent_id, target_agent_id);
        }
        OplogEntry::CardTransferred { target_holder, .. }
        | OplogEntry::CardTransferConfirmed { target_holder, .. } => {
            rewrite_forked_agent_holder(target_holder, source_agent_id, target_agent_id);
        }
        OplogEntry::CardRevokedCascade {
            affected_wallets, ..
        } => {
            for holder in affected_wallets {
                rewrite_forked_agent_holder(holder, source_agent_id, target_agent_id);
            }
        }
        _ => {}
    }
    entry
}

#[async_trait]
impl<Ctx: WorkerCtx> WorkerForkService for DefaultWorkerFork<Ctx> {
    async fn fork_stream_slot(&self, request: ForkStreamSlotRequest) -> ForkStreamSlotResponse {
        export::fork(self, request).await
    }

    async fn fork(
        &self,
        fork_account_id: AccountId,
        source_agent_id: &OwnedAgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerExecutorError> {
        self.perform_fork(
            fork_account_id,
            source_agent_id,
            target_agent_id,
            oplog_index_cut_off,
            None,
            auth_ctx,
        )
        .await
    }

    async fn fork_and_write_fork_result(
        &self,
        fork_account_id: AccountId,
        source_agent_id: &OwnedAgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        copied_scope_start: Option<OplogIndex>,
        forked_phantom_id: Uuid,
        auth_ctx: &AuthCtx,
    ) -> Result<(), WorkerExecutorError> {
        self.perform_fork(
            fork_account_id,
            source_agent_id,
            target_agent_id,
            oplog_index_cut_off,
            Some((copied_scope_start, forked_phantom_id)),
            auth_ctx,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::Principal;
    use golem_common::model::card::{CardId, InvocationWalletPin, WalletVersionToken};
    use golem_common::model::component::ComponentId;
    use golem_common::model::invocation_context::TraceId;
    use golem_common::model::oplog::OplogPayload;
    use golem_common::model::{AgentInvocationPayload, IdempotencyKey};
    use golem_common::schema::SchemaValue;
    use test_r::test;

    fn agent_id(name: &str) -> AgentId {
        AgentId {
            component_id: ComponentId(Uuid::new_v4()),
            agent_id: name.to_string(),
        }
    }

    #[test]
    fn fork_rewrites_invocation_wallet_identity() {
        let source = agent_id("source");
        let target = agent_id("target");
        let entry = OplogEntry::AgentInvocationStarted {
            timestamp: Timestamp::now_utc(),
            idempotency_key: IdempotencyKey::new("fork-wallet-pin".to_string()),
            payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::AgentMethod {
                method_name: "test".to_string(),
                input: SchemaValue::Record { fields: Vec::new() },
                principal: Principal::anonymous(),
                scope_card: None,
            })),
            trace_id: TraceId::generate(),
            trace_states: Vec::new(),
            invocation_context: Vec::new(),
            wallet_pin: Box::new(InvocationWalletPin {
                wallet_token: WalletVersionToken {
                    wallet_id_hash: CardHolder::Agent(AgentCardHolder {
                        agent_id: source.clone(),
                    })
                    .wallet_id_hash(),
                    generation: 7,
                },
                pinned_card_ids: Vec::new(),
                scope_card_id: None,
            }),
        };

        match rewrite_forked_oplog_entry(entry, &source, &target) {
            OplogEntry::AgentInvocationStarted { wallet_pin, .. } => assert_eq!(
                wallet_pin.wallet_token.wallet_id_hash,
                CardHolder::Agent(AgentCardHolder { agent_id: target }).wallet_id_hash()
            ),
            other => panic!("expected pinned invocation start, got {other:?}"),
        }
    }

    #[test]
    fn fork_rewrites_only_local_transfer_holders() {
        let source = agent_id("source");
        let target = agent_id("target");
        let remote = agent_id("remote");
        let entry = OplogEntry::CardTransferStarted {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            transfer_id: Uuid::new_v4(),
            card_id: CardId::new(),
            source_holder: CardHolder::Agent(AgentCardHolder {
                agent_id: source.clone(),
            }),
            target_holder: CardHolder::Agent(AgentCardHolder {
                agent_id: remote.clone(),
            }),
            source_wallet_generation: 8,
        };

        match rewrite_forked_oplog_entry(entry, &source, &target) {
            OplogEntry::CardTransferStarted {
                source_holder,
                target_holder,
                ..
            } => {
                assert_eq!(
                    source_holder,
                    CardHolder::Agent(AgentCardHolder { agent_id: target })
                );
                assert_eq!(
                    target_holder,
                    CardHolder::Agent(AgentCardHolder { agent_id: remote })
                );
            }
            other => panic!("expected transfer start, got {other:?}"),
        }
    }
}
