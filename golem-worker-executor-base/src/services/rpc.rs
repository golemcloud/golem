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

use crate::error::GolemError;
use crate::grpc::{authorised_grpc_request, UriBackConversion};
use crate::services::{
    active_workers, blob_store, component, golem_config, invocation_key, key_value, oplog, promise,
    recovery, scheduler, shard, shard_manager, worker, worker_enumeration, HasActiveWorkers,
    HasBlobStoreService, HasComponentService, HasConfig, HasExtraDeps, HasInvocationKeyService,
    HasKeyValueService, HasOplogService, HasPromiseService, HasRecoveryManagement, HasRpc,
    HasRunningWorkerEnumerationService, HasSchedulerService, HasShardService, HasWasmtimeEngine,
    HasWorkerEnumerationService, HasWorkerService,
};
use crate::worker::{invoke, invoke_and_await, Worker};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use bincode::{Decode, Encode};
use golem_api_grpc::proto::golem::worker::worker_error::Error;
use golem_api_grpc::proto::golem::worker::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::{
    get_invocation_key_response, invoke_and_await_response, invoke_response, CallingConvention,
    GetInvocationKeyRequest, GetInvocationKeyResponse, InvokeAndAwaitRequest,
    InvokeAndAwaitResponse, InvokeParameters, InvokeRequest, InvokeResponse, WorkerError,
};
use golem_common::model::{AccountId, WorkerId};
use golem_wasm_rpc::{Value, WitValue};
use http::Uri;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;
use tracing::debug;
use uuid::Uuid;

#[async_trait]
pub trait Rpc {
    async fn create_demand(&self, worker_id: &WorkerId) -> Box<dyn RpcDemand>;

    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        function_params: Vec<WitValue>,
        account_id: &AccountId,
    ) -> Result<WitValue, RpcError>;

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        function_params: Vec<WitValue>,
        account_id: &AccountId,
    ) -> Result<(), RpcError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
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

impl From<WorkerError> for RpcError {
    fn from(value: WorkerError) -> Self {
        match &value.error {
            Some(Error::BadRequest(errors)) => Self::ProtocolError {
                details: format!("Bad request: {}", errors.errors.join(", ")),
            },
            Some(Error::Unauthorized(error)) => Self::Denied {
                details: format!("Unauthorized: {}", error.error),
            },
            Some(Error::LimitExceeded(error)) => Self::Denied {
                details: format!("Limit exceeded: {}", error.error),
            },
            Some(Error::NotFound(error)) => Self::NotFound {
                details: error.error.clone(),
            },
            Some(Error::AlreadyExists(error)) => Self::ProtocolError {
                details: format!(
                    "Unexpected response: worker already exists: {}",
                    error.error
                ),
            },
            Some(Error::InternalError(error)) => {
                match TryInto::<GolemError>::try_into(error.clone()) {
                    Ok(golem_error) => Self::RemoteInternalError {
                        details: golem_error.to_string(),
                    },
                    Err(_) => Self::ProtocolError {
                        details: format!("Invalid internal error: {:?}", error.error),
                    },
                }
            }
            None => Self::ProtocolError {
                details: "Error response without any details".to_string(),
            },
        }
    }
}

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

pub trait RpcDemand: Send + Sync {}

pub struct RemoteInvocationRpc {
    endpoint: Uri,
    access_token: Uuid,
}

impl RemoteInvocationRpc {
    pub fn new(endpoint: Uri, access_token: Uuid) -> Self {
        Self {
            endpoint,
            access_token,
        }
    }
}

struct LoggingDemand {
    worker_id: WorkerId,
}

impl LoggingDemand {
    pub fn new(worker_id: WorkerId) -> Self {
        log::info!("Initializing RPC connection for worker {}", worker_id);
        Self { worker_id }
    }
}

impl RpcDemand for LoggingDemand {}

impl Drop for LoggingDemand {
    fn drop(&mut self) {
        log::info!("Dropping RPC connection for worker {}", self.worker_id);
    }
}

/// Rpc implementation simply calling the public Golem Worker API for invocation
#[async_trait]
impl Rpc for RemoteInvocationRpc {
    async fn create_demand(&self, worker_id: &WorkerId) -> Box<dyn RpcDemand> {
        let demand = LoggingDemand::new(worker_id.clone());
        Box::new(demand)
    }

    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        function_params: Vec<WitValue>,
        _account_id: &AccountId,
    ) -> Result<WitValue, RpcError> {
        debug!("Invoking remote worker {worker_id} function {function_name} with parameters {function_params:?}");

        let proto_params = function_params
            .into_iter()
            .map(|param| {
                let value: Value = param.into();
                value.into()
            })
            .collect();
        let invoke_parameters = Some(InvokeParameters {
            params: proto_params,
        });

        let mut client = WorkerServiceClient::connect(self.endpoint.as_http_02()).await?;

        let response: GetInvocationKeyResponse = client
            .get_invocation_key(authorised_grpc_request(
                GetInvocationKeyRequest {
                    worker_id: Some(worker_id.clone().into()),
                },
                &self.access_token,
            ))
            .await?
            .into_inner();

        match response.result {
            Some(get_invocation_key_response::Result::Success(invocation_key)) => {
                let response: InvokeAndAwaitResponse = client
                    .invoke_and_await(authorised_grpc_request(
                        InvokeAndAwaitRequest {
                            worker_id: Some(worker_id.clone().into()),
                            invocation_key: Some(invocation_key),
                            function: function_name,
                            invoke_parameters,
                            calling_convention: CallingConvention::Component as i32,
                        },
                        &self.access_token,
                    ))
                    .await?
                    .into_inner();

                match response.result {
                    Some(invoke_and_await_response::Result::Success(result)) => {
                        let mut result_values = Vec::new();
                        for proto_value in result.result {
                            let value: Value =
                                proto_value
                                    .try_into()
                                    .map_err(|err| RpcError::ProtocolError {
                                        details: format!("Could not decode result: {err}"),
                                    })?;
                            result_values.push(value);
                        }
                        let result: WitValue = Value::Tuple(result_values).into();
                        Ok(result)
                    }
                    Some(invoke_and_await_response::Result::Error(error)) => Err(error.into()),
                    None => Err(RpcError::ProtocolError {
                        details: "Empty response through the worker API".to_string(),
                    }),
                }
            }
            Some(get_invocation_key_response::Result::Error(error)) => Err(error.into()),
            None => Err(RpcError::ProtocolError {
                details: "Empty response through the worker API".to_string(),
            }),
        }
    }

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        function_params: Vec<WitValue>,
        _account_id: &AccountId,
    ) -> Result<(), RpcError> {
        debug!("Invoking remote worker {worker_id} function {function_name} with parameters {function_params:?} without awaiting for the result");

        let proto_params = function_params
            .into_iter()
            .map(|param| {
                let value: Value = param.into();
                value.into()
            })
            .collect();
        let invoke_parameters = Some(InvokeParameters {
            params: proto_params,
        });

        let mut client = WorkerServiceClient::connect(self.endpoint.as_http_02()).await?;

        let response: InvokeResponse = client
            .invoke(authorised_grpc_request(
                InvokeRequest {
                    worker_id: Some(worker_id.clone().into()),
                    function: function_name,
                    invoke_parameters,
                },
                &self.access_token,
            ))
            .await?
            .into_inner();

        match response.result {
            Some(invoke_response::Result::Success(_)) => Ok(()),
            Some(invoke_response::Result::Error(error)) => Err(error.into()),
            None => Err(RpcError::ProtocolError {
                details: "Empty response through the worker API".to_string(),
            }),
        }
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
    worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
    worker_enumeration_service: Arc<dyn worker_enumeration::WorkerEnumerationService + Send + Sync>,
    running_worker_enumeration_service:
        Arc<dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync>,
    promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
    golem_config: Arc<golem_config::GolemConfig>,
    invocation_key_service: Arc<dyn invocation_key::InvocationKeyService + Send + Sync>,
    shard_service: Arc<dyn shard::ShardService + Send + Sync>,
    key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
    blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
    oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
    recovery_management: Arc<Mutex<Option<Arc<dyn recovery::RecoveryManagement + Send + Sync>>>>,
    scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
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
            worker_service: self.worker_service.clone(),
            worker_enumeration_service: self.worker_enumeration_service.clone(),
            running_worker_enumeration_service: self.running_worker_enumeration_service.clone(),
            promise_service: self.promise_service.clone(),
            golem_config: self.golem_config.clone(),
            invocation_key_service: self.invocation_key_service.clone(),
            shard_service: self.shard_service.clone(),
            key_value_service: self.key_value_service.clone(),
            blob_store_service: self.blob_store_service.clone(),
            oplog_service: self.oplog_service.clone(),
            recovery_management: self.recovery_management.clone(),
            scheduler_service: self.scheduler_service.clone(),
            extra_deps: self.extra_deps.clone(),
        }
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

impl<Ctx: WorkerCtx> HasInvocationKeyService for DirectWorkerInvocationRpc<Ctx> {
    fn invocation_key_service(
        &self,
    ) -> Arc<dyn invocation_key::InvocationKeyService + Send + Sync> {
        self.invocation_key_service.clone()
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

impl<Ctx: WorkerCtx> HasRecoveryManagement for DirectWorkerInvocationRpc<Ctx> {
    fn recovery_management(&self) -> Arc<dyn recovery::RecoveryManagement + Send + Sync> {
        self.recovery_management
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .clone()
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

impl<Ctx: WorkerCtx> DirectWorkerInvocationRpc<Ctx> {
    pub fn new(
        remote_rpc: Arc<RemoteInvocationRpc>,
        active_workers: Arc<active_workers::ActiveWorkers<Ctx>>,
        engine: Arc<wasmtime::Engine>,
        linker: Arc<wasmtime::component::Linker<Ctx>>,
        runtime: Handle,
        component_service: Arc<dyn component::ComponentService + Send + Sync>,
        worker_service: Arc<dyn worker::WorkerService + Send + Sync>,
        worker_enumeration_service: Arc<
            dyn worker_enumeration::WorkerEnumerationService + Send + Sync,
        >,
        running_worker_enumeration_service: Arc<
            dyn worker_enumeration::RunningWorkerEnumerationService + Send + Sync,
        >,
        promise_service: Arc<dyn promise::PromiseService + Send + Sync>,
        golem_config: Arc<golem_config::GolemConfig>,
        invocation_key_service: Arc<dyn invocation_key::InvocationKeyService + Send + Sync>,
        shard_service: Arc<dyn shard::ShardService + Send + Sync>,
        shard_manager_service: Arc<dyn shard_manager::ShardManagerService + Send + Sync>,
        key_value_service: Arc<dyn key_value::KeyValueService + Send + Sync>,
        blob_store_service: Arc<dyn blob_store::BlobStoreService + Send + Sync>,
        oplog_service: Arc<dyn oplog::OplogService + Send + Sync>,
        scheduler_service: Arc<dyn scheduler::SchedulerService + Send + Sync>,
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
            worker_service,
            worker_enumeration_service,
            running_worker_enumeration_service,
            promise_service,
            golem_config,
            invocation_key_service,
            shard_service,
            key_value_service,
            blob_store_service,
            oplog_service,
            recovery_management: Arc::new(Mutex::new(None)),
            scheduler_service,
            extra_deps,
        }
    }

    pub fn set_recovery_management(
        &self,
        recovery_management: Arc<dyn recovery::RecoveryManagement + Send + Sync>,
    ) {
        *self.recovery_management.lock().unwrap() = Some(recovery_management);
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Rpc for DirectWorkerInvocationRpc<Ctx> {
    async fn create_demand(&self, worker_id: &WorkerId) -> Box<dyn RpcDemand> {
        let demand = LoggingDemand::new(worker_id.clone());
        Box::new(demand)
    }

    async fn invoke_and_await(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        function_params: Vec<WitValue>,
        account_id: &AccountId,
    ) -> Result<WitValue, RpcError> {
        if self.shard_service().check_worker(worker_id).is_ok() {
            debug!("Invoking local worker {worker_id} function {function_name} with parameters {function_params:?}");

            let input_values = function_params
                .into_iter()
                .map(|wit_value| wit_value.into())
                .collect();
            let invocation_key = self.invocation_key_service.generate_key(worker_id);

            let worker =
                Worker::get_or_create(self, worker_id, None, None, None, account_id.clone())
                    .await?;

            let result_values = invoke_and_await(
                worker,
                self,
                invocation_key,
                golem_common::model::CallingConvention::Component,
                function_name,
                input_values,
            )
            .await?;
            Ok(Value::Tuple(result_values).into())
        } else {
            self.remote_rpc
                .invoke_and_await(worker_id, function_name, function_params, account_id)
                .await
        }
    }

    async fn invoke(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        function_params: Vec<WitValue>,
        account_id: &AccountId,
    ) -> Result<(), RpcError> {
        if self.shard_service().check_worker(worker_id).is_ok() {
            debug!("Invoking local worker {worker_id} function {function_name} with parameters {function_params:?} without awaiting for the result");

            let input_values = function_params
                .into_iter()
                .map(|wit_value| wit_value.into())
                .collect();
            let invocation_key = self.invocation_key_service.generate_key(worker_id);

            let worker =
                Worker::get_or_create(self, worker_id, None, None, None, account_id.clone())
                    .await?;

            invoke(
                worker,
                self,
                invocation_key,
                golem_common::model::CallingConvention::Component,
                function_name,
                input_values,
            )
            .await?;
            Ok(())
        } else {
            self.remote_rpc
                .invoke(worker_id, function_name, function_params, account_id)
                .await
        }
    }
}

impl RpcDemand for () {}

#[cfg(any(feature = "mocks", test))]
pub struct RpcMock;

#[cfg(any(feature = "mocks", test))]
impl Default for RpcMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl RpcMock {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl Rpc for RpcMock {
    async fn create_demand(&self, _worker_id: &WorkerId) -> Box<dyn RpcDemand> {
        Box::new(())
    }

    async fn invoke_and_await(
        &self,
        _worker_id: &WorkerId,
        _function_name: String,
        _function_params: Vec<WitValue>,
        _account_id: &AccountId,
    ) -> Result<WitValue, RpcError> {
        todo!()
    }

    async fn invoke(
        &self,
        _worker_id: &WorkerId,
        _function_name: String,
        _function_params: Vec<WitValue>,
        _account_id: &AccountId,
    ) -> Result<(), RpcError> {
        todo!()
    }
}
