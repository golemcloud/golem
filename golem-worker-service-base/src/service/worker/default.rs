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
use golem_wasm_ast::analysis::AnalysedFunctionResult;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::Val as ProtoVal;
use tonic::transport::Channel;
use tonic::Code;
use tracing::{error, info};

use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_api_grpc::proto::golem::worker::{InvocationContext, InvokeResult};
use golem_api_grpc::proto::golem::workerexecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    CompletePromiseRequest, ConnectWorkerRequest, CreateWorkerRequest, InterruptWorkerRequest,
    InvokeAndAwaitWorkerRequest, ResumeWorkerRequest, UpdateWorkerRequest,
};
use golem_common::client::MultiTargetGrpcClient;
use golem_common::config::RetryConfig;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::OplogCursor;
use golem_common::model::{
    AccountId, ComponentId, ComponentVersion, FilterComparator, IdempotencyKey, PromiseId,
    ScanCursor, TargetWorkerId, WorkerFilter, WorkerId, WorkerStatus,
};
use golem_service_base::model::{
    GetOplogResponse, GolemErrorUnknown, ResourceLimits, WorkerMetadata,
};
use golem_service_base::routing_table::HasRoutingTableService;
use golem_service_base::{
    model::{Component, GolemError},
    routing_table::RoutingTableService,
};

use crate::service::component::ComponentService;

use super::{
    AllExecutors, CallWorkerExecutorError, ConnectWorkerStream, HasWorkerExecutorClients,
    RandomExecutor, ResponseMapResult, RoutingLogic, WorkerServiceError,
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

    fn validate_typed_parameters(
        &self,
        params: Vec<TypeAnnotatedValue>,
    ) -> WorkerResult<Vec<ProtoVal>>;

    /// Validates the provided list of `TypeAnnotatedValue` parameters, and then
    /// invokes the worker and waits its results, returning it as a `TypeAnnotatedValue`.
    async fn validate_and_invoke_and_await_typed(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<TypeAnnotatedValue>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<TypeAnnotatedValue> {
        let params = self.validate_typed_parameters(params)?;
        self.invoke_and_await_typed(
            worker_id,
            idempotency_key,
            function_name,
            params,
            invocation_context,
            metadata,
        )
        .await
    }

    /// Invokes a worker using raw `Val` parameter values and awaits its results returning
    /// it as a `TypeAnnotatedValue`.
    async fn invoke_and_await_typed(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<TypeAnnotatedValue>;

    /// Invokes a worker using raw `Val` parameter values and awaits its results returning
    /// a `Val` values (without type information)
    async fn invoke_and_await(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<InvokeResult>;

    /// Validates the provided list of `TypeAnnotatedValue` parameters, and then enqueues
    /// an invocation for the worker without awaiting its results.
    async fn validate_and_invoke(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<TypeAnnotatedValue>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<()> {
        let params = self.validate_typed_parameters(params)?;
        self.invoke(
            worker_id,
            idempotency_key,
            function_name,
            params,
            invocation_context,
            metadata,
        )
        .await
    }

    /// Enqueues an invocation for the worker without awaiting its results, using raw `Val`
    /// parameters.
    async fn invoke(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
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

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        metadata: WorkerRequestMetadata,
        auth_ctx: &AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError>;
}

pub struct TypedResult {
    pub result: TypeAnnotatedValue,
    pub function_result_types: Vec<AnalysedFunctionResult>,
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
            WorkerServiceError::InternalCallError,
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
        let worker_id_err: WorkerId = worker_id.clone();
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
                |error| match error {
                    CallWorkerExecutorError::FailedToConnectToPod(status)
                        if status.code() == Code::NotFound =>
                    {
                        WorkerServiceError::WorkerNotFound(worker_id_err.clone())
                    }
                    _ => WorkerServiceError::InternalCallError(error),
                },
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
            WorkerServiceError::InternalCallError,
        )
        .await?;

        Ok(())
    }

    fn validate_typed_parameters(
        &self,
        params: Vec<TypeAnnotatedValue>,
    ) -> WorkerResult<Vec<ProtoVal>> {
        let mut result = Vec::new();
        for param in params {
            result.push(golem_wasm_rpc::protobuf::Val::from(
                golem_wasm_rpc::Value::try_from(param).map_err(WorkerServiceError::TypeChecker)?,
            ));
        }
        Ok(result)
    }

    async fn invoke_and_await_typed(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<TypeAnnotatedValue> {
        let worker_id = worker_id.clone();
        let worker_id_clone = worker_id.clone();
        let function_name_clone = function_name.clone();

        let invoke_response = self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                info!("Invoking function on {}: {}", worker_id_clone, function_name);
                Box::pin(worker_executor_client.invoke_and_await_worker_typed(
                    InvokeAndAwaitWorkerRequest {
                        worker_id: Some(worker_id_clone.clone().into()),
                        name: function_name.clone(),
                        input: params.clone(),
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
            WorkerServiceError::InternalCallError,
        ).await?;

        Ok(invoke_response)
    }

    async fn invoke_and_await(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
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
                        idempotency_key: idempotency_key.clone().map(|k| k.into()),
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
            WorkerServiceError::InternalCallError,
        ).await?;

        Ok(invoke_response)
    }

    async fn invoke(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
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
                        idempotency_key: idempotency_key.clone().map(|k| k.into()),
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
            WorkerServiceError::InternalCallError,
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
            oplog_idx: OplogIndex::from_u64(oplog_id),
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
                WorkerServiceError::InternalCallError,
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
            WorkerServiceError::InternalCallError,
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
            WorkerServiceError::InternalCallError,
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
            WorkerServiceError::InternalCallError,
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
            WorkerServiceError::InternalCallError,
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

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        metadata: WorkerRequestMetadata,
        _auth_ctx: &AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let worker_id = worker_id.clone();
        self.call_worker_executor(
            worker_id.clone(),
            move |worker_executor_client| {
                info!("Get oplog");
                let worker_id = worker_id.clone();
                Box::pin(
                    worker_executor_client.get_oplog(workerexecutor::v1::GetOplogRequest {
                        worker_id: Some(worker_id.into()),
                        from_oplog_index: from_oplog_index.into(),
                        cursor: cursor.clone().map(|c| c.into()),
                        count,
                        account_id: metadata.account_id.clone().map(|id| id.into()),
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::GetOplogResponse {
                    result:
                        Some(workerexecutor::v1::get_oplog_response::Result::Success(
                            workerexecutor::v1::GetOplogSuccessResponse {
                                entries,
                                next,
                                first_index_in_chunk,
                                last_index,
                            },
                        )),
                } => Ok(GetOplogResponse {
                    entries: entries
                        .into_iter()
                        .map(|e| e.try_into())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|err| {
                            GolemError::Unknown(GolemErrorUnknown {
                                details: format!("Unexpected oplog entries in error: {err}"),
                            })
                        })?,
                    next: next.map(|c| c.into()),
                    first_index_in_chunk,
                    last_index,
                }),
                workerexecutor::v1::GetOplogResponse {
                    result: Some(workerexecutor::v1::get_oplog_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::GetOplogResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
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
            WorkerServiceError::InternalCallError,
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
                        workerexecutor::v1::GetWorkersMetadataRequest {
                            component_id: Some(component_id),
                            filter: filter.clone().map(|f| f.into()),
                            cursor: Some(cursor.clone().into()),
                            count,
                            precise,
                            account_id,
                        },
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
                            .map_err(|err| {
                                GolemError::Unknown(GolemErrorUnknown {
                                    details: format!(
                                        "Unexpected worker metadata in response: {err}"
                                    ),
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
                WorkerServiceError::InternalCallError,
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
