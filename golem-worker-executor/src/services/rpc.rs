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

use super::file_loader::FileLoader;
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
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_common::model::{IdempotencyKey, OwnedWorkerId, WorkerId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm::{ValueAndType, WitValue};
use rib::{ParsedFunctionName, ParsedFunctionSite};
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
        self_created_by: &AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<Box<dyn RpcDemand>, RpcError>;

    async fn invoke_and_await(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        self_created_by: &AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<Option<ValueAndType>, RpcError>;

    async fn invoke(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        self_created_by: &AccountId,
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

impl From<golem_wasm::RpcError> for RpcError {
    fn from(value: golem_wasm::RpcError) -> Self {
        match value {
            golem_wasm::RpcError::ProtocolError(details) => Self::ProtocolError { details },
            golem_wasm::RpcError::Denied(details) => Self::Denied { details },
            golem_wasm::RpcError::NotFound(details) => Self::NotFound { details },
            golem_wasm::RpcError::RemoteInternalError(details) => {
                Self::RemoteInternalError { details }
            }
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
        self_created_by: &AccountId,
        _self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        _self_stack: InvocationContextStack, // TODO: make invocation context propagating through the worker start API
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        debug!("Ensuring remote target worker exists");

        let demand = LoggingDemand::new(owned_worker_id.worker_id());

        self.worker_proxy
            .start(
                owned_worker_id,
                HashMap::from_iter(self_env.to_vec()),
                self_config,
                self_created_by,
            )
            .await?;

        Ok(Box::new(demand))
    }

    async fn invoke_and_await(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        self_created_by: &AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<Option<ValueAndType>, RpcError> {
        Ok(self
            .worker_proxy
            .invoke_and_await(
                owned_worker_id,
                idempotency_key,
                function_name,
                function_params,
                self_worker_id.clone(),
                HashMap::from_iter(self_env.to_vec()),
                self_config,
                self_stack,
                self_created_by,
            )
            .await?)
    }

    async fn invoke(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        function_params: Vec<WitValue>,
        self_created_by: &AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<(), RpcError> {
        Ok(self
            .worker_proxy
            .invoke(
                owned_worker_id,
                idempotency_key,
                function_name,
                function_params,
                self_worker_id.clone(),
                HashMap::from_iter(self_env.to_vec()),
                self_config,
                self_stack,
                self_created_by,
            )
            .await?)
    }
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
            extra_deps,
        }
    }

    /// As we know the target component's metadata, and it includes the root package name, we can
    /// accept function names which are not fully qualified by falling back to use this root package
    /// when the package part is missing.
    async fn enrich_function_name(
        &self,
        target_worker_id: &OwnedWorkerId,
        function_name: String,
    ) -> String {
        let parsed_function_name: Option<ParsedFunctionName> =
            ParsedFunctionName::parse(&function_name).ok();
        if matches!(
            parsed_function_name,
            Some(ParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: _
            }) | Some(ParsedFunctionName {
                site: ParsedFunctionSite::PackagedInterface { .. },
                function: _
            })
        ) {
            // already valid function name, doing nothing
            function_name
        } else if let Ok(target_component) = self
            .component_service
            .get_metadata(&target_worker_id.worker_id.component_id, None)
            .await
        {
            enrich_function_name_by_target_information(
                function_name,
                target_component.metadata.root_package_name().clone(),
                target_component.metadata.root_package_version().clone(),
            )
        } else {
            // If we cannot get the target metadata, we just go with the original function name
            // and let it fail on that.
            function_name
        }
    }
}

fn enrich_function_name_by_target_information(
    function_name: String,
    root_package_name: Option<String>,
    root_package_version: Option<String>,
) -> String {
    if let Some(root_package_name) = root_package_name {
        // Hack for supporting the 'unique' name generation of golem-test-framework. Stripping everything after '---'
        let root_package_name = strip_unique_suffix(&root_package_name);

        if let Some(root_package_version) = root_package_version {
            // The target root package is versioned, and the version has to be put _after_ the interface
            // name which we assume to be the first section (before a dot) of the provided string:

            if let Some((interface_name, rest)) = function_name.split_once('.') {
                let enriched_function_name =
                    format!("{root_package_name}/{interface_name}@{root_package_version}.{rest}");
                if ParsedFunctionName::parse(&enriched_function_name).is_ok() {
                    enriched_function_name
                } else {
                    // If the enriched function name is still not valid, we just return the original function name
                    function_name
                }
            } else {
                // Unexpected format, we just return the original function name
                function_name
            }
        } else {
            // The target root package is not versioned, so we can just simply prefix the root package name
            // to the provided function name and see if it is valid:
            let enriched_function_name = format!("{root_package_name}/{function_name}");
            if ParsedFunctionName::parse(&enriched_function_name).is_ok() {
                enriched_function_name
            } else {
                // If the enriched function name is still not valid, we just return the original function name
                function_name
            }
        }
    } else {
        // No root package information in the target, we can't do anything
        function_name
    }
}

fn strip_unique_suffix(root_package_name: &str) -> String {
    if let Some(index) = root_package_name.rfind("---") {
        root_package_name[..index].to_string()
    } else {
        root_package_name.to_string()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Rpc for DirectWorkerInvocationRpc<Ctx> {
    async fn create_demand(
        &self,
        owned_worker_id: &OwnedWorkerId,
        self_created_by: &AccountId,
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
        function_name: String,
        function_params: Vec<WitValue>,
        self_created_by: &AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<Option<ValueAndType>, RpcError> {
        let idempotency_key = idempotency_key.unwrap_or(IdempotencyKey::fresh());
        let function_name = self
            .enrich_function_name(owned_worker_id, function_name)
            .await;

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
                self_created_by,
                owned_worker_id,
                Some(self_env.to_vec()),
                Some(self_config),
                None,
                Some(self_worker_id.clone()),
                &self_stack,
            )
            .await?;

            let result_value = worker
                .invoke_and_await(idempotency_key, function_name, input_values, self_stack)
                .await?;

            Ok(result_value)
        } else {
            self.remote_rpc
                .invoke_and_await(
                    owned_worker_id,
                    Some(idempotency_key),
                    function_name,
                    function_params,
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
        function_name: String,
        function_params: Vec<WitValue>,
        self_created_by: &AccountId,
        self_worker_id: &WorkerId,
        self_env: &[(String, String)],
        self_config: BTreeMap<String, String>,
        self_stack: InvocationContextStack,
    ) -> Result<(), RpcError> {
        let idempotency_key = idempotency_key.unwrap_or(IdempotencyKey::fresh());
        let function_name = self
            .enrich_function_name(owned_worker_id, function_name)
            .await;

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
                self_created_by,
                owned_worker_id,
                Some(self_env.to_vec()),
                Some(self_config),
                None,
                Some(self_worker_id.clone()),
                &self_stack,
            )
            .await?;

            worker
                .invoke(idempotency_key, function_name, input_values, self_stack)
                .await?;
            Ok(())
        } else {
            self.remote_rpc
                .invoke(
                    owned_worker_id,
                    Some(idempotency_key),
                    function_name,
                    function_params,
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

#[cfg(test)]
mod tests {
    use crate::services::rpc::enrich_function_name_by_target_information;
    use test_r::test;

    #[test]
    fn test_enrich_function_name_by_target_information() {
        assert_eq!(
            enrich_function_name_by_target_information("api.{x}".to_string(), None, None),
            "api.{x}".to_string()
        );
        assert_eq!(
            enrich_function_name_by_target_information(
                "api.{x}".to_string(),
                Some("test:pkg".to_string()),
                None
            ),
            "test:pkg/api.{x}".to_string()
        );
        assert_eq!(
            enrich_function_name_by_target_information(
                "api.{x}".to_string(),
                Some("test:pkg".to_string()),
                Some("1.0.0".to_string())
            ),
            "test:pkg/api@1.0.0.{x}".to_string()
        );
        assert_eq!(
            enrich_function_name_by_target_information(
                "run".to_string(),
                Some("test:pkg".to_string()),
                Some("1.0.0".to_string())
            ),
            "run".to_string()
        );
    }
}
