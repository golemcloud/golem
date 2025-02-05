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

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

use async_trait::async_trait;
use bincode::{Decode, Encode};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::WitValue;
use tokio::runtime::Handle;
use tracing::debug;

use super::file_loader::FileLoader;
use crate::error::GolemError;
use crate::services::events::Events;
use crate::services::oplog::plugin::OplogProcessorPlugin;
use crate::services::plugins::Plugins;
use crate::services::shard::ShardService;
use crate::services::worker_proxy::{WorkerProxy, WorkerProxyError};
use crate::services::{
    active_workers, blob_store, component, golem_config, key_value, oplog, promise, scheduler,
    shard, shard_manager, worker, worker_activator, worker_enumeration, worker_fork,
    HasActiveWorkers, HasBlobStoreService, HasComponentService, HasConfig, HasEvents, HasExtraDeps,
    HasFileLoader, HasKeyValueService, HasOplogProcessorPlugin, HasOplogService, HasPlugins,
    HasPromiseService, HasRpc, HasRunningWorkerEnumerationService, HasSchedulerService,
    HasShardManagerService, HasShardService, HasWasmtimeEngine, HasWorkerActivator,
    HasWorkerEnumerationService, HasWorkerForkService, HasWorkerProxy, HasWorkerService,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use golem_common::model::component::ComponentOwner;
use golem_common::model::{IdempotencyKey, OwnedWorkerId, TargetWorkerId, WorkerId};

#[async_trait]
pub trait Rpc {
    async fn create_demand(&self, owned_worker_id: &OwnedWorkerId) -> Box<dyn RpcDemand>;

    async fn invoke_and_await(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        self_worker_id: &WorkerId,
        self_args: &[String],
        self_env: &[(String, String)],
    ) -> Result<TypeAnnotatedValue, RpcError>;

    async fn invoke(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        self_worker_id: &WorkerId,
        self_args: &[String],
        self_env: &[(String, String)],
    ) -> Result<(), RpcError>;

    async fn generate_unique_local_worker_id(
        &self,
        target_worker_id: TargetWorkerId,
    ) -> Result<WorkerId, GolemError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum RpcError {
    ProtocolError { details: String },
    Denied { details: String },
    NotFound { details: String },
    RemoteInternalError { details: String },
}

impl Display for RpcError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::ProtocolError { details } => write!(f, "Protocol error: {}", details),
            RpcError::Denied { details } => write!(f, "Denied: {}", details),
            RpcError::NotFound { details } => write!(f, "Not found: {}", details),
            RpcError::RemoteInternalError { details } => {
                write!(f, "Remote internal error: {}", details)
            }
        }
    }
}

impl std::error::Error for RpcError {}

impl From<tonic::transport::Error> for RpcError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::ProtocolError {
            details: format!("gRPC Transport error: {}", value),
        }
    }
}

impl From<tonic::Status> for RpcError {
    fn from(value: tonic::Status) -> Self {
        Self::ProtocolError {
            details: format!("gRPC error: {}", value),
        }
    }
}

impl From<GolemError> for RpcError {
    fn from(value: GolemError) -> Self {
        match value {
            GolemError::WorkerAlreadyExists { worker_id } => RpcError::Denied {
                details: format!("Worker {worker_id} already exists"),
            },
            GolemError::WorkerNotFound { worker_id } => RpcError::NotFound {
                details: format!("Worker {worker_id} not found"),
            },
            GolemError::InvalidAccount => RpcError::Denied {
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

impl From<golem_wasm_rpc::RpcError> for RpcError {
    fn from(value: golem_wasm_rpc::RpcError) -> Self {
        match value {
            golem_wasm_rpc::RpcError::ProtocolError(details) => Self::ProtocolError { details },
            golem_wasm_rpc::RpcError::Denied(details) => Self::Denied { details },
            golem_wasm_rpc::RpcError::NotFound(details) => Self::NotFound { details },
            golem_wasm_rpc::RpcError::RemoteInternalError(details) => {
                Self::RemoteInternalError { details }
            }
        }
    }
}

pub trait RpcDemand: Send + Sync {}

pub struct RemoteInvocationRpc {
    worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
    shard_service: Arc<dyn ShardService + Send + Sync>,
}

impl RemoteInvocationRpc {
    pub fn new(
        worker_proxy: Arc<dyn WorkerProxy + Send + Sync>,
        shard_service: Arc<dyn ShardService + Send + Sync>,
    ) -> Self {
        Self {
            worker_proxy,
            shard_service,
        }
    }
}

struct LoggingDemand {
    worker_id: WorkerId,
}

impl LoggingDemand {
    pub fn new(worker_id: WorkerId) -> Self {
        log::debug!("Initializing RPC connection for worker {}", worker_id);
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
    async fn create_demand(&self, owned_worker_id: &OwnedWorkerId) -> Box<dyn RpcDemand> {
        let demand = LoggingDemand::new(owned_worker_id.worker_id());
        Box::new(demand)
    }

    async fn invoke_and_await(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        self_worker_id: &WorkerId,
        self_args: &[String],
        self_env: &[(String, String)],
    ) -> Result<TypeAnnotatedValue, RpcError> {
        Ok(self
            .worker_proxy
            .invoke_and_await(
                owned_worker_id,
                idempotency_key,
                function_name,
                function_params,
                self_worker_id.clone(),
                self_args.to_vec(),
                HashMap::from_iter(self_env.to_vec()),
            )
            .await?)
    }

    async fn invoke(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        self_worker_id: &WorkerId,
        self_args: &[String],
        self_env: &[(String, String)],
    ) -> Result<(), RpcError> {
        Ok(self
            .worker_proxy
            .invoke(
                owned_worker_id,
                idempotency_key,
                function_name,
                function_params,
                self_worker_id.clone(),
                self_args.to_vec(),
                HashMap::from_iter(self_env.to_vec()),
            )
            .await?)
    }

    async fn generate_unique_local_worker_id(
        &self,
        target_worker_id: TargetWorkerId,
    ) -> Result<WorkerId, GolemError> {
        let current_assignment = self.shard_service.current_assignment()?;
        Ok(target_worker_id.into_worker_id(
            &current_assignment.shard_ids,
            current_assignment.number_of_shards,
        ))
    }
}

pub struct DirectWorkerInvocationRpc<Ctx: WorkerCtx> {
    remote_rpc: Arc<RemoteInvocationRpc>,
    active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
    engine: Arc<wasmtime::Engine>,
    linker: Arc<wasmtime::component::Linker<Ctx>>,
    runtime: Handle,
    component_service: Arc<dyn component::ComponentService + Send + Sync>,
    shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
    worker_fork: Arc<dyn worker_fork::WorkerForkService + Send + Sync>,
    worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>,
    running_worker_enumeration_service:
        Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync>,
    promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
    golem_config: Arc<golem_config::GolemConfig>,
    shard_service: Arc<dyn shard::ShardService + Send + Sync>,
    key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
    oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
    scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
    worker_activator: Arc<dyn worker_activator::WorkerActivator<Ctx> + Send + Sync>,
    events: Arc<Events>,
    file_loader: Arc<FileLoader>,
    plugins: Arc<
        dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
            + Send
            + Sync,
    >,
    oplog_processor_plugin: Arc<dyn OplogProcessorPlugin + Send + Sync>,
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
            oplog_service: self.oplog_service.clone(),
            scheduler_service: self.scheduler_service.clone(),
            worker_activator: self.worker_activator.clone(),
            events: self.events.clone(),
            file_loader: self.file_loader.clone(),
            plugins: self.plugins.clone(),
            oplog_processor_plugin: self.oplog_processor_plugin.clone(),
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

impl<Ctx: WorkerCtx> HasComponentService for DirectWorkerInvocationRpc<Ctx> {
    fn component_service(&self) -> Arc<dyn component::ComponentService + Send + Sync> {
        self.component_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasConfig for DirectWorkerInvocationRpc<Ctx> {
    fn config(&self) -> Arc<golem_config::GolemConfig> {
        self.golem_config.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerService for DirectWorkerInvocationRpc<Ctx> {
    fn worker_service(&self) -> Arc<dyn worker::WorkerService + Send + Sync> {
        self.worker_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerEnumerationService for DirectWorkerInvocationRpc<Ctx> {
    fn worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync> {
        self.worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasRunningWorkerEnumerationService for DirectWorkerInvocationRpc<Ctx> {
    fn running_worker_enumeration_service(
        &self,
    ) -> Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync> {
        self.running_worker_enumeration_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasPromiseService for DirectWorkerInvocationRpc<Ctx> {
    fn promise_service(&self) -> Arc<dyn promise::PromiseService + Send + Sync> {
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
    fn key_value_service(&self) -> Arc<dyn key_value::KeyValueService + Send + Sync> {
        self.key_value_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasBlobStoreService for DirectWorkerInvocationRpc<Ctx> {
    fn blob_store_service(&self) -> Arc<dyn blob_store::BlobStoreService + Send + Sync> {
        self.blob_store_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasSchedulerService for DirectWorkerInvocationRpc<Ctx> {
    fn scheduler_service(&self) -> Arc<dyn scheduler::SchedulerService + Send + Sync> {
        self.scheduler_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogService for DirectWorkerInvocationRpc<Ctx> {
    fn oplog_service(&self) -> Arc<dyn oplog::OplogService + Send + Sync> {
        self.oplog_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerForkService for DirectWorkerInvocationRpc<Ctx> {
    fn worker_fork_service(&self) -> Arc<dyn worker_fork::WorkerForkService + Send + Sync> {
        self.worker_fork.clone()
    }
}

impl<Ctx: WorkerCtx> HasRpc for DirectWorkerInvocationRpc<Ctx> {
    fn rpc(&self) -> Arc<dyn Rpc + Send + Sync> {
        Arc::new(self.clone())
    }
}

impl<Ctx: WorkerCtx> HasExtraDeps<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn extra_deps(&self) -> Ctx::ExtraDeps {
        self.extra_deps.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardService for DirectWorkerInvocationRpc<Ctx> {
    fn shard_service(&self) -> Arc<dyn shard::ShardService + Send + Sync> {
        self.shard_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasShardManagerService for DirectWorkerInvocationRpc<Ctx> {
    fn shard_manager_service(&self) -> Arc<dyn shard_manager::ShardManagerService + Send + Sync> {
        self.shard_manager_service.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerActivator<Ctx> for DirectWorkerInvocationRpc<Ctx> {
    fn worker_activator(&self) -> Arc<dyn worker_activator::WorkerActivator<Ctx> + Send + Sync> {
        self.worker_activator.clone()
    }
}

impl<Ctx: WorkerCtx> HasWorkerProxy for DirectWorkerInvocationRpc<Ctx> {
    fn worker_proxy(&self) -> Arc<dyn WorkerProxy + Send + Sync> {
        self.remote_rpc.worker_proxy.clone()
    }
}

impl<Ctx: WorkerCtx> HasFileLoader for DirectWorkerInvocationRpc<Ctx> {
    fn file_loader(&self) -> Arc<FileLoader> {
        self.file_loader.clone()
    }
}

impl<Ctx: WorkerCtx>
    HasPlugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
    for DirectWorkerInvocationRpc<Ctx>
{
    fn plugins(
        &self,
    ) -> Arc<
        dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
            + Send
            + Sync,
    > {
        self.plugins.clone()
    }
}

impl<Ctx: WorkerCtx> HasOplogProcessorPlugin for DirectWorkerInvocationRpc<Ctx> {
    fn oplog_processor_plugin(&self) -> Arc<dyn OplogProcessorPlugin + Send + Sync> {
        self.oplog_processor_plugin.clone()
    }
}

impl<Ctx: WorkerCtx> DirectWorkerInvocationRpc<Ctx> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        remote_rpc: Arc<RemoteInvocationRpc>,
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        component_service: Arc<dyn component::ComponentService + Send + Sync>,
        worker_fork: Arc<dyn worker_fork::WorkerForkService + Send + Sync>,
        worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync,
        >,
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        golem_config: Arc<golem_config::GolemConfig>,
        shard_service: Arc<dyn shard::ShardService + Send + Sync>,
        shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
        oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
        scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
        worker_activator: Arc<dyn worker_activator::WorkerActivator<Ctx> + Send + Sync>,
        events: Arc<Events>,
        file_loader: Arc<FileLoader>,
        plugins: Arc<
            dyn Plugins<<Ctx::ComponentOwner as ComponentOwner>::PluginOwner, Ctx::PluginScope>
                + Send
                + Sync,
        >,
        oplog_processor_plugin: Arc<dyn OplogProcessorPlugin + Send + Sync>,
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
            oplog_service,
            scheduler_service,
            worker_activator,
            events,
            file_loader,
            plugins,
            oplog_processor_plugin,
            extra_deps,
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Rpc for DirectWorkerInvocationRpc<Ctx> {
    async fn create_demand(&self, owned_worker_id: &OwnedWorkerId) -> Box<dyn RpcDemand> {
        let demand = LoggingDemand::new(owned_worker_id.worker_id());
        Box::new(demand)
    }

    async fn invoke_and_await(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        self_worker_id: &WorkerId,
        self_args: &[String],
        self_env: &[(String, String)],
    ) -> Result<TypeAnnotatedValue, RpcError> {
        let idempotency_key = idempotency_key.unwrap_or(IdempotencyKey::fresh());

        if self
            .shard_service()
            .check_worker(&owned_worker_id.worker_id)
            .is_ok()
        {
            debug!("Invoking local worker function {function_name} with parameters {function_params:?}");

            let input_values = function_params
                .into_iter()
                .map(|wit_value| wit_value.into())
                .collect();

            let worker = Worker::get_or_create_running(
                self,
                owned_worker_id,
                Some(self_args.to_vec()),
                Some(self_env.to_vec()),
                None,
                Some(self_worker_id.clone()),
            )
            .await?;

            let result_values = worker
                .invoke_and_await(idempotency_key, function_name, input_values)
                .await?;

            Ok(result_values)
        } else {
            self.remote_rpc
                .invoke_and_await(
                    owned_worker_id,
                    Some(idempotency_key),
                    function_name,
                    function_params,
                    self_worker_id,
                    self_args,
                    self_env,
                )
                .await
        }
    }

    async fn invoke(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        self_worker_id: &WorkerId,
        self_args: &[String],
        self_env: &[(String, String)],
    ) -> Result<(), RpcError> {
        let idempotency_key = idempotency_key.unwrap_or(IdempotencyKey::fresh()); // TODO

        if self
            .shard_service()
            .check_worker(&owned_worker_id.worker_id())
            .is_ok()
        {
            debug!("Invoking local worker function {function_name} with parameters {function_params:?} without awaiting for the result");

            let input_values = function_params
                .into_iter()
                .map(|wit_value| wit_value.into())
                .collect();

            let worker = Worker::get_or_create_running(
                self,
                owned_worker_id,
                Some(self_args.to_vec()),
                Some(self_env.to_vec()),
                None,
                Some(self_worker_id.clone()),
            )
            .await?;

            worker
                .invoke(idempotency_key, function_name, input_values)
                .await?;
            Ok(())
        } else {
            self.remote_rpc
                .invoke(
                    owned_worker_id,
                    Some(idempotency_key),
                    function_name,
                    function_params,
                    self_worker_id,
                    self_args,
                    self_env,
                )
                .await
        }
    }

    async fn generate_unique_local_worker_id(
        &self,
        target_worker_id: TargetWorkerId,
    ) -> Result<WorkerId, GolemError> {
        self.remote_rpc
            .generate_unique_local_worker_id(target_worker_id)
            .await
    }
}

impl RpcDemand for () {}
