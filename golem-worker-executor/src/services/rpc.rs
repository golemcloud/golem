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

use super::agent_webhooks::AgentWebhooksService;
use super::file_loader::FileLoader;
use super::HasAgentWebhooksService;
use crate::services::events::Events;
use crate::services::oplog::plugin::OplogProcessorPlugin;
use crate::services::resource_limits::ResourceLimits;
use crate::services::shard::ShardService;
use crate::services::worker_proxy::{WorkerProxy, WorkerProxyError};
use crate::services::{
    active_workers, agent_types, blob_store, component, golem_config, key_value, oplog, promise,
    rdbms, scheduler, shard_manager, worker, worker_activator, worker_enumeration, worker_fork,
    HasActiveWorkers, HasAgentTypesService, HasBlobStoreService, HasComponentService, HasConfig,
    HasEvents, HasExtraDeps, HasFileLoader, HasKeyValueService, HasOplogProcessorPlugin,
    HasOplogService, HasPromiseService, HasRdbmsService, HasResourceLimits, HasRpc,
    HasRunningWorkerEnumerationService, HasSchedulerService, HasShardManagerService,
    HasShardService, HasWasmtimeEngine, HasWorkerActivator, HasWorkerEnumerationService,
    HasWorkerForkService, HasWorkerProxy, HasWorkerService,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentInvocationMode, AgentPrincipal, Principal, UntypedDataValue,
};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_common::model::{IdempotencyKey, OwnedWorkerId, WorkerId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use tokio::runtime::Handle;
use tracing::debug;

#[async_trait]
pub trait Rpc: Send + Sync {
    async fn create_demand(
        &self,
        owned_worker_id: &OwnedWorkerId,
        self_created_by: AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<Box<dyn RpcDemand>, RpcError>;

    async fn invoke_and_await(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        method_name: String,
        method_parameters: UntypedDataValue,
        self_created_by: AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<UntypedDataValue, RpcError>;

    async fn invoke(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        method_name: String,
        method_parameters: UntypedDataValue,
        self_created_by: AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<(), RpcError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcError {
    ProtocolError { details: String },
    Denied { details: String },
    NotFound { details: String },
    RemoteInternalError { details: String },
}

impl From<SerializableRpcError> for RpcError {
    fn from(value: SerializableRpcError) -> Self {
        match value {
            SerializableRpcError::ProtocolError { details } => Self::ProtocolError { details },
            SerializableRpcError::Denied { details } => Self::Denied { details },
            SerializableRpcError::NotFound { details } => Self::NotFound { details },
            SerializableRpcError::RemoteInternalError { details } => {
                Self::RemoteInternalError { details }
            }
        }
    }
}

impl From<RpcError> for SerializableRpcError {
    fn from(value: RpcError) -> Self {
        match value {
            RpcError::ProtocolError { details } => SerializableRpcError::ProtocolError { details },
            RpcError::Denied { details } => SerializableRpcError::Denied { details },
            RpcError::NotFound { details } => SerializableRpcError::NotFound { details },
            RpcError::RemoteInternalError { details } => {
                SerializableRpcError::RemoteInternalError { details }
            }
        }
    }
}

impl Display for RpcError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::ProtocolError { details } => write!(f, "Protocol error: {details}"),
            RpcError::Denied { details } => write!(f, "Denied: {details}"),
            RpcError::NotFound { details } => write!(f, "Not found: {details}"),
            RpcError::RemoteInternalError { details } => {
                write!(f, "Remote internal error: {details}")
            }
        }
    }
}

impl std::error::Error for RpcError {}

impl From<tonic::transport::Error> for RpcError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::ProtocolError {
            details: format!("gRPC Transport error: {value}"),
        }
    }
}

impl From<tonic::Status> for RpcError {
    fn from(value: tonic::Status) -> Self {
        Self::ProtocolError {
            details: format!("gRPC error: {value}"),
        }
    }
}

impl From<WorkerExecutorError> for RpcError {
    fn from(value: WorkerExecutorError) -> Self {
        match value {
            WorkerExecutorError::WorkerAlreadyExists { worker_id } => RpcError::Denied {
                details: format!("Worker {worker_id} already exists"),
            },
            WorkerExecutorError::WorkerNotFound { worker_id } => RpcError::NotFound {
                details: format!("Worker {worker_id} not found"),
            },
            WorkerExecutorError::InvalidAccount => RpcError::Denied {
                details: "Invalid account".to_string(),
            },
            _ => RpcError::RemoteInternalError {
                details: value.to_string(),
            },
        }
    }
}

impl From<WorkerProxyError> for RpcError {
    fn from(value: WorkerProxyError) -> Self {
        match value {
            WorkerProxyError::BadRequest(errors) => RpcError::ProtocolError {
                details: errors.join(", "),
            },
            WorkerProxyError::Unauthorized(error) => RpcError::Denied { details: error },
            WorkerProxyError::LimitExceeded(error) => RpcError::Denied { details: error },
            WorkerProxyError::NotFound(error) => RpcError::NotFound { details: error },
            WorkerProxyError::AlreadyExists(error) => RpcError::Denied { details: error },
            WorkerProxyError::InternalError(error) => error.into(),
        }
    }
}

impl From<crate::preview2::golem::agent::host::RpcError> for RpcError {
    fn from(value: crate::preview2::golem::agent::host::RpcError) -> Self {
        use crate::preview2::golem::agent::host::RpcError as WitRpcError;
        match value {
            WitRpcError::ProtocolError(details) => Self::ProtocolError { details },
            WitRpcError::Denied(details) => Self::Denied { details },
            WitRpcError::NotFound(details) => Self::NotFound { details },
            WitRpcError::RemoteInternalError(details) => Self::RemoteInternalError { details },
            WitRpcError::RemoteAgentError(err) => Self::RemoteInternalError {
                details: format!("{err:?}"),
            },
        }
    }
}

impl From<RpcError> for crate::preview2::golem::agent::host::RpcError {
    fn from(value: RpcError) -> Self {
        match value {
            RpcError::ProtocolError { details } => Self::ProtocolError(details),
            RpcError::Denied { details } => Self::Denied(details),
            RpcError::NotFound { details } => Self::NotFound(details),
            RpcError::RemoteInternalError { details } => Self::RemoteInternalError(details),
        }
    }
}

pub trait RpcDemand: Send + Sync {}

pub struct RemoteInvocationRpc {
    worker_proxy: Arc<dyn WorkerProxy>,
    _shard_service: Arc<dyn ShardService>,
}

impl RemoteInvocationRpc {
    pub fn new(worker_proxy: Arc<dyn WorkerProxy>, shard_service: Arc<dyn ShardService>) -> Self {
        Self {
            worker_proxy,
            _shard_service: shard_service,
        }
    }
}

struct LoggingDemand {
    worker_id: WorkerId,
}

impl LoggingDemand {
    pub fn new(worker_id: WorkerId) -> Self {
        log::debug!("Initializing RPC connection for worker {worker_id}");
        Self { worker_id }
    }
}

impl RpcDemand for LoggingDemand {}

impl Drop for LoggingDemand {
    fn drop(&mut self) {
        log::debug!("Dropping RPC connection for worker {}", self.worker_id);
    }
}

/// Rpc implementation simply calling the public Golem Worker API for invocation
#[async_trait]
impl Rpc for RemoteInvocationRpc {
    async fn create_demand(
        &self,
        owned_worker_id: &OwnedWorkerId,
        self_created_by: AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        debug!("Ensuring remote target worker exists");

        let principal = caller_agent_principal(self_worker_id);
        let demand = LoggingDemand::new(owned_worker_id.worker_id());

        self.worker_proxy
            .start(
                owned_worker_id,
                self_worker_id,
                HashMap::from_iter(self_env.to_vec()),
                self_config,
                self_stack,
                self_created_by,
                principal,
            )
            .await?;

        Ok(Box::new(demand))
    }

    async fn invoke_and_await(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        method_name: String,
        method_parameters: UntypedDataValue,
        self_created_by: AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<UntypedDataValue, RpcError> {
        let principal = caller_agent_principal(self_worker_id);

        let result = self
            .worker_proxy
            .invoke_agent(
                &owned_worker_id.worker_id(),
                method_name,
                method_parameters,
                AgentInvocationMode::Await,
                None,
                idempotency_key,
                self_worker_id.clone(),
                HashMap::from_iter(self_env.to_vec()),
                self_config,
                self_stack,
                self_created_by,
                principal,
            )
            .await?;

        result.ok_or_else(|| RpcError::RemoteInternalError {
            details: "Expected a result from agent invoke_and_await but got None".to_string(),
        })
    }

    async fn invoke(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        method_name: String,
        method_parameters: UntypedDataValue,
        self_created_by: AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<(), RpcError> {
        let principal = caller_agent_principal(self_worker_id);

        self.worker_proxy
            .invoke_agent(
                &owned_worker_id.worker_id(),
                method_name,
                method_parameters,
                AgentInvocationMode::Schedule,
                None,
                idempotency_key,
                self_worker_id.clone(),
                HashMap::from_iter(self_env.to_vec()),
                self_config,
                self_stack,
                self_created_by,
                principal,
            )
            .await?;

        Ok(())
    }
}

fn caller_agent_principal(self_worker_id: &WorkerId) -> Principal {
    Principal::Agent(AgentPrincipal {
        agent_id: self_worker_id.clone(),
    })
}

pub struct DirectWorkerInvocationRpc<Ctx: WorkerCtx> {
    remote_rpc: Arc<RemoteInvocationRpc>,
    active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
    engine: Arc<wasmtime::Engine>,
    linker: Arc<wasmtime::component::Linker<Ctx>>,
    runtime: Handle,
    component_service: Arc<dyn component::ComponentService>,
    shard_manager_service: Arc<dyn shard_manager::ShardManagerService>,
    worker_fork: Arc<dyn worker_fork::WorkerForkService>,
    worker_service: Arc<dyn worker::WorkerService>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
    running_worker_enumeration_service:
        Arc<dyn worker_enumeration::RunningWorkerEnumerationService>,
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
    agent_types_service: Arc<dyn agent_types::AgentTypesService>,
    agent_webhooks_service: Arc<AgentWebhooksService>,
    extra_deps: Ctx::ExtraDeps,
}

impl<Ctx: WorkerCtx> Clone for DirectWorkerInvocationRpc<Ctx> {
    fn clone(&self) -> Self {
        Self {
            remote_rpc: self.remote_rpc.clone(),
            active_workers: self.active_workers.clone(),
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            runtime: self.runtime.clone(),
            component_service: self.component_service.clone(),
            shard_manager_service: self.shard_manager_service.clone(),
            worker_fork: self.worker_fork.clone(),
            worker_service: self.worker_service.clone(),
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
            agent_types_service: self.agent_types_service.clone(),
            agent_webhooks_service: self.agent_webhooks_service.clone(),
            extra_deps: self.extra_deps.clone(),
        }
    }
}

impl<Ctx: WorkerCtx> HasEvents for DirectWorkerInvocationRpc<Ctx> {
    fn events(&self) -> Arc<Events> {
        self.events.clone()
    }
}

impl<Ctx: WorkerCtx> HasActiveWorkers<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn active_workers(&self) -> Arc<active_workers::ActiveWorkers<Ctx>> {
        self.active_workers.clone()
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

impl<Ctx: WorkerCtx> HasComponentService for DirectWorkerInvocationRpc<Ctx> {
    fn component_service(&self) -> Arc<dyn component::ComponentService> {
        self.component_service.clone()
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

#[allow(clippy::too_many_arguments)]
impl<Ctx: WorkerCtx> DirectWorkerInvocationRpc<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        remote_rpc: Arc<RemoteInvocationRpc>,
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        component_service: Arc<dyn component::ComponentService>,
        worker_fork: Arc<dyn worker_fork::WorkerForkService>,
        worker_service: Arc<dyn worker::WorkerService>,
        worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService>,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService,
        >,
        promise_service: Arc<dyn promise::PromiseService>,
        golem_config: Arc<golem_config::GolemConfig>,
        shard_service: Arc<dyn ShardService>,
        shard_manager_service: Arc<dyn shard_manager::ShardManagerService>,
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
        agent_types_service: Arc<dyn agent_types::AgentTypesService>,
        agent_webhooks_service: Arc<AgentWebhooksService>,
        extra_deps: Ctx::ExtraDeps,
    ) -> Self {
        Self {
            remote_rpc,
            active_workers,
            engine,
            linker,
            runtime,
            component_service,
            shard_manager_service,
            worker_fork,
            worker_service,
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
            agent_types_service,
            agent_webhooks_service,
            extra_deps,
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Rpc for DirectWorkerInvocationRpc<Ctx> {
    async fn create_demand(
        &self,
        owned_worker_id: &OwnedWorkerId,
        self_created_by: AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        if self
            .shard_service()
            .check_worker(&owned_worker_id.worker_id)
            .is_ok()
        {
            debug!(target_worker_id = %owned_worker_id, "Ensuring local target worker exists");

            let _worker = Worker::get_or_create_running(
                self,
                self_created_by,
                owned_worker_id,
                Some(self_env.to_vec()),
                Some(self_config),
                None,
                Some(self_worker_id.clone()),
                &self_stack,
                Principal::Agent(AgentPrincipal {
                    agent_id: self_worker_id.clone(),
                }),
            )
            .await?;

            let demand = LoggingDemand::new(owned_worker_id.worker_id());
            Ok(Box::new(demand))
        } else {
            self.remote_rpc
                .create_demand(
                    owned_worker_id,
                    self_created_by,
                    self_worker_id,
                    self_env,
                    self_config,
                    self_stack,
                )
                .await
        }
    }

    async fn invoke_and_await(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        method_name: String,
        method_parameters: UntypedDataValue,
        self_created_by: AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<UntypedDataValue, RpcError> {
        if self
            .shard_service()
            .check_worker(&owned_worker_id.worker_id)
            .is_ok()
        {
            // TODO: implement local direct invocation path for agent-based invocation, use Worker::get_or_create_running
            unimplemented!()
        } else {
            self.remote_rpc
                .invoke_and_await(
                    owned_worker_id,
                    Some(idempotency_key.unwrap_or(IdempotencyKey::fresh())),
                    method_name,
                    method_parameters,
                    self_created_by,
                    self_worker_id,
                    self_env,
                    self_config,
                    self_stack,
                )
                .await
        }
    }

    async fn invoke(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        method_name: String,
        method_parameters: UntypedDataValue,
        self_created_by: AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<(), RpcError> {
        if self
            .shard_service()
            .check_worker(&owned_worker_id.worker_id)
            .is_ok()
        {
            // TODO: implement local direct invocation path for agent-based invocation, use Worker::get_or_create_running
            unimplemented!()
        } else {
            self.remote_rpc
                .invoke(
                    owned_worker_id,
                    Some(idempotency_key.unwrap_or(IdempotencyKey::fresh())),
                    method_name,
                    method_parameters,
                    self_created_by,
                    self_worker_id,
                    self_env,
                    self_config,
                    self_stack,
                )
                .await
        }
    }
}

impl RpcDemand for () {}
