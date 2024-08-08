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

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::{TypedTuple, Val as ProtoVal};
use poem_openapi::types::ToJSON;
use tonic::transport::Channel;
use tracing::{error, info};

use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_api_grpc::proto::golem::worker::{
    IdempotencyKey as ProtoIdempotencyKey, InvocationContext, InvokeResult,
};
use golem_api_grpc::proto::golem::workerexecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    CompletePromiseRequest, ConnectWorkerRequest, CreateWorkerRequest, InterruptWorkerRequest,
    InvokeAndAwaitWorkerRequest, ResumeWorkerRequest, UpdateWorkerRequest,
};
use golem_common::client::MultiTargetGrpcClient;
use golem_common::config::RetryConfig;
use golem_common::model::exports::FunctionResult;
use golem_common::model::precise_json::PreciseJson;
use golem_common::model::{
    AccountId, ComponentId, ComponentVersion, FilterComparator, IdempotencyKey, ScanCursor,
    Timestamp, WorkerFilter, WorkerStatus,
};
use golem_service_base::model::{
    GolemErrorUnknown, PromiseId, ResourceLimits, WorkerId, WorkerMetadata,
};
use golem_service_base::routing_table::HasRoutingTableService;
use golem_service_base::{
    model::{Component, GolemError},
    routing_table::RoutingTableService,
};

use crate::service::component::ComponentService;

use super::{
    AllExecutors, ConnectWorkerStream, HasWorkerExecutorClients, RandomExecutor, ResponseMapResult,
    RoutingLogic, WorkerServiceError,
};

pub type WorkerResult<T> = Result<T, WorkerServiceError>;

#[async_trait]
pub trait WorkerService<AuthCtx> {
    async fn create(
        &self,
        worker_id: &WorkerId,
        component_version: u64,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<WorkerId>;

    async fn connect(
        &self,
        worker_id: &WorkerId,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<ConnectWorkerStream>;

    async fn delete(
        &self,
        worker_id: &WorkerId,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<()>;

    // Accepts Json Params and Returns Json
    async fn invoke_and_await_function_json(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<PreciseJson>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<PreciseJson> {
        let result = self
            .invoke_and_await_function_typed(
                worker_id,
                idempotency_key,
                function_name,
                params,
                calling_convention,
                invocation_context,
                metadata,
            )
            .await?;
        Ok(result.into())
    }

    // Accepts Json Params and Returns TypeAnnotatedValue
    async fn invoke_and_await_function_typed(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<PreciseJson>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<TypeAnnotatedValue>;

    // Accepts a Vec<Val> and returns a Vec<Val> (with no type information)
    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<InvokeResult>;

    // Accepts Json parameters as input
    async fn invoke_function_json(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<PreciseJson>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()>;

    // Accepts Vec<Val> as input
    async fn invoke_function_proto(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()>;

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<bool>;

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<()>;

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<WorkerMetadata>;

    async fn find_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<WorkerMetadata>)>;

    async fn resume(
        &self,
        worker_id: &WorkerId,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<()>;

    async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: UpdateMode,
        target_version: ComponentVersion,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<()>;

    async fn get_component_for_worker(
        &self,
        worker_id: &WorkerId,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> Result<Component, WorkerServiceError>;
}

pub struct TypedResult {
    pub result: TypeAnnotatedValue,
    pub function_result_types: Vec<FunctionResult>,
}

#[derive(Clone, Debug)]
pub struct WorkerRequestMetadata {
    pub account_id: Option<AccountId>,
    pub limits: Option<ResourceLimits>,
}

#[derive(Clone)]
pub struct WorkerServiceDefault<AuthCtx> {
    worker_executor_clients: MultiTargetGrpcClient<WorkerExecutorClient<Channel>>,
    // NOTE: unlike other retries, reaching max_attempts for the worker executor
    //       (with retryable errors) does not end the retry loop,
    //       rather it emits a warn log and resets the retry state.
    worker_executor_retries: RetryConfig,
    component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
    routing_table_service: Arc<dyn RoutingTableService + Send + Sync>,
}

impl<AuthCtx> WorkerServiceDefault<AuthCtx> {
    pub fn new(
        worker_executor_clients: MultiTargetGrpcClient<WorkerExecutorClient<Channel>>,
        worker_executor_retries: RetryConfig,
        component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
        routing_table_service: Arc<dyn RoutingTableService + Send + Sync>,
    ) -> Self {
        Self {
            worker_executor_clients,
            worker_executor_retries,
            component_service,
            routing_table_service,
        }
    }
}

impl<AuthCtx> HasRoutingTableService for WorkerServiceDefault<AuthCtx> {
    fn routing_table_service(&self) -> &Arc<dyn RoutingTableService + Send + Sync> {
        &self.routing_table_service
    }
}

impl<AuthCtx> HasWorkerExecutorClients for WorkerServiceDefault<AuthCtx> {
    fn worker_executor_clients(&self) -> &MultiTargetGrpcClient<WorkerExecutorClient<Channel>> {
        &self.worker_executor_clients
    }

    fn worker_executor_retry_config(&self) -> &RetryConfig {
        &self.worker_executor_retries
    }
}

#[async_trait]
impl<AuthCtx> WorkerService<AuthCtx> for WorkerServiceDefault<AuthCtx>
where
    AuthCtx: Send + Sync,
{
    async fn create(
        &self,
        worker_id: &WorkerId,
        component_version: u64,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<WorkerId> {
        let worker_id_clone = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                info!("Create worker");
                let worker_id = worker_id_clone.clone();
                Box::pin(worker_executor_client.create_worker(CreateWorkerRequest {
                    worker_id: Some(worker_id.into()),
                    component_version,
                    args: arguments.clone(),
                    env: environment_variables.clone(),
                    account_id: metadata.account_id.clone().map(|id| id.into()),
                    account_limits: metadata.limits.clone().map(|id| id.into()),
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::CreateWorkerResponse {
                    result: Some(workerexecutor::v1::create_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::CreateWorkerResponse {
                    result: Some(workerexecutor::v1::create_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::CreateWorkerResponse { .. } => Err("Empty response".into()),
            },
        )
        .await?;

        Ok(worker_id.clone())
    }

    async fn connect(
        &self,
        worker_id: &WorkerId,
        metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<ConnectWorkerStream> {
        let worker_id = worker_id.clone();
        let stream = self
            .call_worker_executor(
                worker_id.clone(),
                move |worker_executor_client| {
                    info!("Connect worker");
                    Box::pin(worker_executor_client.connect_worker(ConnectWorkerRequest {
                        worker_id: Some(worker_id.clone().into()),
                        account_id: metadata.account_id.clone().map(|id| id.into()),
                        account_limits: metadata.limits.clone().map(|id| id.into()),
                    }))
                },
                |response| Ok(ConnectWorkerStream::new(response.into_inner())),
            )
            .await?;

        Ok(stream)
    }

    async fn delete(
        &self,
        worker_id: &WorkerId,
        metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                info!("Delete worker");
                let worker_id = worker_id.clone();
                Box::pin(worker_executor_client.delete_worker(
                    workerexecutor::v1::DeleteWorkerRequest {
                        worker_id: Some(golem_api_grpc::proto::golem::worker::WorkerId::from(
                            worker_id.clone(),
                        )),
                        account_id: metadata.account_id.clone().map(|id| id.into()),
                    },
                ))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::DeleteWorkerResponse {
                    result: Some(workerexecutor::v1::delete_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::DeleteWorkerResponse {
                    result: Some(workerexecutor::v1::delete_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::DeleteWorkerResponse { .. } => Err("Empty response".into()),
            },
        )
        .await?;

        Ok(())
    }

    async fn invoke_and_await_function_typed(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<PreciseJson>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<TypeAnnotatedValue> {
        let worker_id = worker_id.clone();
        let worker_id_clone = worker_id.clone();
        let function_name_clone = function_name.clone();

        let params_: Vec<golem_wasm_rpc::protobuf::Val> = params
            .into_iter()
            .map(|v| golem_wasm_rpc::protobuf::Val::from(golem_wasm_rpc::Value::from(v)))
            .collect::<Vec<_>>();

        let invoke_response = self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                info!("Invoking function on {}: {}", worker_id_clone, function_name);
                Box::pin(worker_executor_client.invoke_and_await_worker_typed(
                    InvokeAndAwaitWorkerRequest {
                        worker_id: Some(worker_id_clone.clone().into()),
                        name: function_name.clone(),
                        input: params_.clone(),
                        idempotency_key: idempotency_key.clone().map(|v| v.into()),
                        account_id: metadata.account_id.clone().map(|id| id.into()),
                        account_limits: metadata.limits.clone().map(|id| id.into()),
                        context: invocation_context.clone(),
                    }
                )
                )
            },
            move |response| {
                match response.into_inner() {
                    workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result:
                        Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Success(
                                 workerexecutor::v1::InvokeAndAwaitWorkerSuccessTyped {
                                     output: Some(output),
                                 },
                             )),
                    } => {
                        info!("Invoked function on {}: {}", worker_id, function_name_clone);
                        output.type_annotated_value.ok_or("Empty response".into())
                    }
                    workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result:
                        Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Failure(err)),
                    } => {
                        error!("Invoked function on {}: {} failed with {err:?}", worker_id, function_name_clone);
                        Err(err.into())
                    }
                    workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped { .. } => {
                        error!("Invoked function on {}: {} failed with empty response", worker_id, function_name_clone);
                        Err("Empty response".into())
                    }
                }
            },
        ).await?;

        Ok(invoke_response)
    }

    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<InvokeResult> {
        let worker_id = worker_id.clone();
        let worker_id_clone = worker_id.clone();

        let invoke_response = self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                info!("Invoke and await function");
                Box::pin(worker_executor_client.invoke_and_await_worker(
                    workerexecutor::v1::InvokeAndAwaitWorkerRequest {
                        worker_id: Some(worker_id_clone.clone().into()),
                        name: function_name.clone(),
                        input: params.clone(),
                        idempotency_key: idempotency_key.clone(),
                        account_id: metadata.account_id.clone().map(|id| id.into()),
                        account_limits: metadata.limits.clone().map(|id| id.into()),
                        context: invocation_context.clone(),
                    }
                )
                )
            },
            move |response| {
                match response.into_inner() {
                    workerexecutor::v1::InvokeAndAwaitWorkerResponse {
                        result:
                        Some(workerexecutor::v1::invoke_and_await_worker_response::Result::Success(
                                 workerexecutor::v1::InvokeAndAwaitWorkerSuccess {
                                     output,
                                 },
                             )),
                    } => {
                        Ok(InvokeResult { result: output })
                    }
                    workerexecutor::v1::InvokeAndAwaitWorkerResponse {
                        result:
                        Some(workerexecutor::v1::invoke_and_await_worker_response::Result::Failure(err)),
                    } => {
                        error!("Invoked function error: {err:?}");
                        Err(err.into())
                    }
                    workerexecutor::v1::InvokeAndAwaitWorkerResponse { .. } => {
                        error!("Invoked function failed with empty response");
                        Err("Empty response".into())
                    }
                }
            },
        ).await?;

        Ok(invoke_response)
    }

    async fn invoke_function_json(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<PreciseJson>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();

        let params_: Vec<golem_wasm_rpc::protobuf::Val> = params
            .into_iter()
            .map(|v| golem_wasm_rpc::protobuf::Val::from(golem_wasm_rpc::Value::from(v)))
            .collect::<Vec<_>>();

        self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(worker_executor_client.invoke_worker(
                    workerexecutor::v1::InvokeWorkerRequest {
                        worker_id: Some(worker_id.into()),
                        name: function_name.clone(),
                        input: params_.clone(),
                        idempotency_key: idempotency_key.clone().map(|v| v.into()),
                        account_id: metadata.account_id.clone().map(|id| id.into()),
                        account_limits: metadata.limits.clone().map(|id| id.into()),
                        context: invocation_context.clone(),
                    },
                ))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::InvokeWorkerResponse {
                    result: Some(workerexecutor::v1::invoke_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::InvokeWorkerResponse {
                    result: Some(workerexecutor::v1::invoke_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::InvokeWorkerResponse { .. } => Err("Empty response".into()),
            },
        )
        .await?;
        Ok(())
    }

    async fn invoke_function_proto(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                info!("Invoke function");
                let worker_id = worker_id.clone();
                Box::pin(worker_executor_client.invoke_worker(
                    workerexecutor::v1::InvokeWorkerRequest {
                        worker_id: Some(worker_id.into()),
                        idempotency_key: idempotency_key.clone(),
                        name: function_name.clone(),
                        input: params.clone(),
                        account_id: metadata.account_id.clone().map(|id| id.into()),
                        account_limits: metadata.limits.clone().map(|id| id.into()),
                        context: invocation_context.clone(),
                    },
                ))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::InvokeWorkerResponse {
                    result: Some(workerexecutor::v1::invoke_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::InvokeWorkerResponse {
                    result: Some(workerexecutor::v1::invoke_worker_response::Result::Failure(err)),
                } => {
                    error!("Invoked function error: {err:?}");
                    Err(err.into())
                }
                workerexecutor::v1::InvokeWorkerResponse { .. } => Err("Empty response".into()),
            },
        )
        .await?;
        Ok(())
    }

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<bool> {
        let promise_id = PromiseId {
            worker_id: worker_id.clone(),
            oplog_idx: oplog_id,
        };

        let result = self
            .call_worker_executor(
                worker_id.clone(),
                move |worker_executor_client| {
                    info!("Complete promise");
                    let promise_id = promise_id.clone();
                    let data = data.clone();
                    Box::pin(
                        worker_executor_client
                            .complete_promise(CompletePromiseRequest {
                                promise_id: Some(promise_id.into()),
                                data,
                                account_id: metadata.account_id.clone().map(|id| id.into()),
                            })
                    )
                },
                |response| {
                    match response.into_inner() {
                        workerexecutor::v1::CompletePromiseResponse {
                            result:
                            Some(workerexecutor::v1::complete_promise_response::Result::Success(
                                     success,
                                 )),
                        } => Ok(success.completed),
                        workerexecutor::v1::CompletePromiseResponse {
                            result:
                            Some(workerexecutor::v1::complete_promise_response::Result::Failure(
                                     err,
                                 )),
                        } => Err(err.into()),
                        workerexecutor::v1::CompletePromiseResponse { .. } => {
                            Err("Empty response".into())
                        }
                    }
                },
            )
            .await?;
        Ok(result)
    }

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                info!("Interrupt");
                let worker_id = worker_id.clone();
                Box::pin(
                    worker_executor_client.interrupt_worker(InterruptWorkerRequest {
                        worker_id: Some(worker_id.into()),
                        recover_immediately,
                        account_id: metadata.account_id.clone().map(|id| id.into()),
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::InterruptWorkerResponse {
                    result: Some(workerexecutor::v1::interrupt_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::InterruptWorkerResponse {
                    result:
                        Some(workerexecutor::v1::interrupt_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::InterruptWorkerResponse { .. } => Err("Empty response".into()),
            },
        )
        .await?;

        Ok(())
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<WorkerMetadata> {
        let worker_id = worker_id.clone();
        let metadata = self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                info!("Get metadata");
                Box::pin(worker_executor_client.get_worker_metadata(
                    workerexecutor::v1::GetWorkerMetadataRequest {
                        worker_id: Some(golem_api_grpc::proto::golem::worker::WorkerId::from(worker_id)),
                        account_id: metadata.account_id.clone().map(|id| id.into()),
                    }
                ))
            },
            |response| {
                match response.into_inner() {
                    workerexecutor::v1::GetWorkerMetadataResponse {
                        result:
                        Some(workerexecutor::v1::get_worker_metadata_response::Result::Success(metadata)),
                    } => {
                        Ok(metadata.try_into().unwrap())
                    }
                    workerexecutor::v1::GetWorkerMetadataResponse {
                        result:
                        Some(workerexecutor::v1::get_worker_metadata_response::Result::Failure(err)),
                    } => {
                        error!("Get metadata error: {err:?}");
                        Err(err.into())
                    }
                    workerexecutor::v1::GetWorkerMetadataResponse { .. } => {
                        Err("Empty response".into())
                    }
                }
            },
        ).await?;

        Ok(metadata)
    }

    async fn find_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<WorkerMetadata>)> {
        info!("Find metadata");
        if filter.as_ref().is_some_and(is_filter_with_running_status) {
            let result = self
                .find_running_metadata_internal(component_id, filter, auth_ctx)
                .await?;

            Ok((None, result.into_iter().take(count as usize).collect()))
        } else {
            self.find_metadata_internal(
                component_id,
                filter,
                cursor,
                count,
                precise,
                metadata,
                auth_ctx,
            )
            .await
        }
    }

    async fn resume(
        &self,
        worker_id: &WorkerId,
        metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                let worker_id = worker_id.clone();
                Box::pin(worker_executor_client.resume_worker(ResumeWorkerRequest {
                    worker_id: Some(worker_id.into()),
                    account_id: metadata.account_id.clone().map(|id| id.into()),
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::ResumeWorkerResponse {
                    result: Some(workerexecutor::v1::resume_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::ResumeWorkerResponse {
                    result: Some(workerexecutor::v1::resume_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::ResumeWorkerResponse { .. } => Err("Empty response".into()),
            },
        )
        .await?;
        Ok(())
    }

    async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: UpdateMode,
        target_version: ComponentVersion,
        metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<()> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                info!("Update worker");
                let worker_id = worker_id.clone();
                Box::pin(worker_executor_client.update_worker(UpdateWorkerRequest {
                    worker_id: Some(worker_id.into()),
                    mode: update_mode.into(),
                    target_version,
                    account_id: metadata.account_id.clone().map(|id| id.into()),
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::UpdateWorkerResponse {
                    result: Some(workerexecutor::v1::update_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::UpdateWorkerResponse {
                    result: Some(workerexecutor::v1::update_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::UpdateWorkerResponse { .. } => Err("Empty response".into()),
            },
        )
        .await?;
        Ok(())
    }

    async fn get_component_for_worker(
        &self,
        worker_id: &WorkerId,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> Result<Component, WorkerServiceError> {
        self.try_get_component_for_worker(worker_id, metadata, auth_ctx)
            .await
    }
}

impl<AuthCtx> WorkerServiceDefault<AuthCtx>
where
    AuthCtx: Send + Sync,
{
    async fn try_get_component_for_worker(
        &self,
        worker_id: &WorkerId,
        request_metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> Result<Component, WorkerServiceError> {
        match self
            .get_metadata(worker_id, request_metadata, auth_ctx)
            .await
        {
            Ok(metadata) => {
                let component_version = metadata.component_version;
                let component_details = self
                    .component_service
                    .get_by_version(&worker_id.component_id, component_version, auth_ctx)
                    .await?;

                Ok(component_details)
            }
            Err(WorkerServiceError::WorkerNotFound(_)) => Ok(self
                .component_service
                .get_latest(&worker_id.component_id, auth_ctx)
                .await?),
            Err(WorkerServiceError::Golem(GolemError::WorkerNotFound(_))) => Ok(self
                .component_service
                .get_latest(&worker_id.component_id, auth_ctx)
                .await?),
            Err(other) => Err(other),
        }
    }

    async fn find_running_metadata_internal(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<Vec<WorkerMetadata>> {
        let component_id = component_id.clone();
        let result = self.call_worker_executor(
            AllExecutors,
            move |worker_executor_client| {
                let component_id: golem_api_grpc::proto::golem::component::ComponentId =
                    component_id.clone().into();

                Box::pin(
                    worker_executor_client.get_running_workers_metadata(
                        workerexecutor::v1::GetRunningWorkersMetadataRequest {
                            component_id: Some(component_id),
                            filter: filter.clone().map(|f| f.into()),
                        }
                    )
                )
            },
            |responses| {
                responses.into_iter().map(|response| {
                    match response.into_inner() {
                        workerexecutor::v1::GetRunningWorkersMetadataResponse {
                            result:
                            Some(workerexecutor::v1::get_running_workers_metadata_response::Result::Success(workerexecutor::v1::GetRunningWorkersMetadataSuccessResponse {
                                                                                                                workers
                                                                                                            })),
                        } => {
                            let workers: Vec<WorkerMetadata> = workers.into_iter().map(|w| w.try_into()).collect::<Result<Vec<_>, _>>().map_err(|_| GolemError::Unknown(GolemErrorUnknown {
                                details: "Convert response error".to_string(),
                            }))?;
                            Ok(workers)
                        }
                        workerexecutor::v1::GetRunningWorkersMetadataResponse {
                            result:
                            Some(workerexecutor::v1::get_running_workers_metadata_response::Result::Failure(err)),
                        } => Err(err.into()),
                        workerexecutor::v1::GetRunningWorkersMetadataResponse { .. } => {
                            Err("Empty response".into())
                        }
                    }
                }).collect::<Result<Vec<_>, ResponseMapResult>>()
            },
        ).await?;

        Ok(result.into_iter().flatten().collect())
    }

    async fn find_metadata_internal(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<WorkerMetadata>)> {
        let component_id = component_id.clone();
        let result = self
            .call_worker_executor(
                RandomExecutor,
                move |worker_executor_client| {
                    let component_id: golem_api_grpc::proto::golem::component::ComponentId =
                        component_id.clone().into();
                    let account_id = metadata.account_id.clone().map(|id| id.into());
                    Box::pin(worker_executor_client.get_workers_metadata(
                    golem_api_grpc::proto::golem::workerexecutor::v1::GetWorkersMetadataRequest {
                        component_id: Some(component_id),
                        filter: filter.clone().map(|f| f.into()),
                        cursor: Some(cursor.clone().into()),
                        count,
                        precise,
                        account_id,
                    }
                ))
                },
                |response| match response.into_inner() {
                    workerexecutor::v1::GetWorkersMetadataResponse {
                        result:
                            Some(workerexecutor::v1::get_workers_metadata_response::Result::Success(
                                workerexecutor::v1::GetWorkersMetadataSuccessResponse {
                                    workers,
                                    cursor,
                                },
                            )),
                    } => {
                        let workers = workers
                            .into_iter()
                            .map(|w| w.try_into())
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|_| {
                                GolemError::Unknown(GolemErrorUnknown {
                                    details: "Convert response error".to_string(),
                                })
                            })?;
                        Ok((cursor.map(|c| c.into()), workers))
                    }
                    workerexecutor::v1::GetWorkersMetadataResponse {
                        result:
                            Some(workerexecutor::v1::get_workers_metadata_response::Result::Failure(
                                err,
                            )),
                    } => Err(err.into()),
                    workerexecutor::v1::GetWorkersMetadataResponse { .. } => {
                        Err("Empty response".into())
                    }
                },
            )
            .await?;

        Ok(result)
    }
}

fn is_filter_with_running_status(filter: &WorkerFilter) -> bool {
    match filter {
        WorkerFilter::Status(f)
            if f.value == WorkerStatus::Running && f.comparator == FilterComparator::Equal =>
        {
            true
        }
        WorkerFilter::And(f) => f.filters.iter().any(is_filter_with_running_status),
        _ => false,
    }
}

#[derive(Clone, Debug)]
pub struct WorkerServiceNoOp {
    pub metadata: WorkerRequestMetadata,
}

#[async_trait]
impl<AuthCtx> WorkerService<AuthCtx> for WorkerServiceNoOp
where
    AuthCtx: Send + Sync,
{
    async fn create(
        &self,
        _worker_id: &WorkerId,
        _component_version: u64,
        _arguments: Vec<String>,
        _environment_variables: HashMap<String, String>,
        _metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<WorkerId> {
        Ok(WorkerId::new(ComponentId::new_v4(), "no-op".to_string()).unwrap())
    }

    async fn connect(
        &self,
        _worker_id: &WorkerId,
        _metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<ConnectWorkerStream> {
        Err(WorkerServiceError::Internal(anyhow::Error::msg(
            "Not supported",
        )))
    }

    async fn delete(
        &self,
        _worker_id: &WorkerId,
        _metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn invoke_and_await_function_typed(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: Option<IdempotencyKey>,
        _function_name: String,
        _params: Vec<PreciseJson>,
        _invocation_context: Option<InvocationContext>,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<TypeAnnotatedValue> {
        Ok(TypeAnnotatedValue::Tuple(TypedTuple {
            value: vec![],
            typ: vec![],
        }))
    }

    async fn invoke_and_await_function_proto(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: Option<ProtoIdempotencyKey>,
        _function_name: String,
        _params: Vec<ProtoVal>,
        _invocation_context: Option<InvocationContext>,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<InvokeResult> {
        Ok(InvokeResult::default())
    }

    async fn invoke_function_json(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: Option<IdempotencyKey>,
        _function_name: String,
        _params: Vec<PreciseJson>,
        _invocation_context: Option<InvocationContext>,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn invoke_function_proto(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: Option<ProtoIdempotencyKey>,
        _function_name: String,
        _params: Vec<ProtoVal>,
        _invocation_context: Option<InvocationContext>,
        _metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn complete_promise(
        &self,
        _worker_id: &WorkerId,
        _oplog_id: u64,
        _data: Vec<u8>,
        _metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<bool> {
        Ok(true)
    }

    async fn interrupt(
        &self,
        _worker_id: &WorkerId,
        _recover_immediately: bool,
        _metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        _metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<WorkerMetadata> {
        Ok(WorkerMetadata {
            worker_id: worker_id.clone(),
            args: vec![],
            env: Default::default(),
            status: WorkerStatus::Running,
            component_version: 0,
            retry_count: 0,
            pending_invocation_count: 0,
            updates: vec![],
            created_at: Timestamp::now_utc(),
            last_error: None,
            component_size: 0,
            total_linear_memory_size: 0,
            owned_resources: HashMap::new(),
        })
    }

    async fn find_metadata(
        &self,
        _component_id: &ComponentId,
        _filter: Option<WorkerFilter>,
        _cursor: ScanCursor,
        _count: u64,
        _precise: bool,
        _metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<WorkerMetadata>)> {
        Ok((None, vec![]))
    }

    async fn resume(
        &self,
        _worker_id: &WorkerId,
        _metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn update(
        &self,
        _worker_id: &WorkerId,
        _update_mode: UpdateMode,
        _target_version: ComponentVersion,
        _metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<()> {
        Ok(())
    }

    async fn get_component_for_worker(
        &self,
        worker_id: &WorkerId,
        _metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> WorkerResult<Component> {
        let worker_id = golem_common::model::WorkerId {
            component_id: worker_id.component_id.clone(),
            worker_name: worker_id.worker_name.to_json_string(),
        };
        Err(WorkerServiceError::WorkerNotFound(worker_id))
    }
}
