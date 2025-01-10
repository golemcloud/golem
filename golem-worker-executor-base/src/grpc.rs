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

use crate::error::*;
use futures_util::Stream;
use futures_util::StreamExt;
use gethostname::gethostname;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::common::ResourceLimits as GrpcResourceLimits;
use golem_api_grpc::proto::golem::worker::{Cursor, ResourceMetadata, UpdateMode};
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_server::WorkerExecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ActivatePluginRequest, ActivatePluginResponse, ConnectWorkerRequest, DeactivatePluginRequest,
    DeactivatePluginResponse, DeleteWorkerRequest, ForkWorkerRequest, ForkWorkerResponse,
    GetFileContentsRequest, GetFileContentsResponse, GetOplogRequest, GetOplogResponse,
    GetRunningWorkersMetadataRequest, GetRunningWorkersMetadataResponse, GetWorkersMetadataRequest,
    GetWorkersMetadataResponse, InvokeAndAwaitWorkerRequest, InvokeAndAwaitWorkerResponseTyped,
    InvokeAndAwaitWorkerSuccess, ListDirectoryRequest, ListDirectoryResponse, SearchOplogRequest,
    SearchOplogResponse, UpdateWorkerRequest, UpdateWorkerResponse,
};
use golem_common::grpc::{
    proto_account_id_string, proto_component_id_string, proto_idempotency_key_string,
    proto_plugin_installation_id_string, proto_promise_id_string, proto_target_worker_id_string,
    proto_worker_id_string,
};
use golem_common::metrics::api::record_new_grpc_api_active_stream;
use golem_common::model::oplog::{OplogIndex, UpdateDescription};
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentId, ComponentType, IdempotencyKey, OwnedWorkerId,
    PluginInstallationId, ScanCursor, ShardId, TargetWorkerId, TimestampedWorkerInvocation,
    WorkerEvent, WorkerFilter, WorkerId, WorkerInvocation, WorkerMetadata, WorkerStatus,
    WorkerStatusRecord,
};
use golem_common::{model as common_model, recorded_grpc_api_request};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::Val;
use std::cmp::min;
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};
use tracing::info_span;
use tracing::{debug, error, info, warn, Instrument};
use uuid::Uuid;
use wasmtime::Error;

use crate::model::public_oplog::{
    find_component_version_at, get_public_oplog_chunk, search_public_oplog,
};
use crate::model::{InterruptKind, LastError, ListDirectoryResult, ReadFileResult};
use crate::services::events::Event;
use crate::services::worker_activator::{DefaultWorkerActivator, LazyWorkerActivator};
use crate::services::worker_event::WorkerEventReceiver;
use crate::services::{
    All, HasActiveWorkers, HasAll, HasComponentService, HasEvents, HasOplogService, HasPlugins,
    HasPromiseService, HasRunningWorkerEnumerationService, HasShardManagerService, HasShardService,
    HasWorkerEnumerationService, HasWorkerService, UsesAllDeps,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use tokio;

pub enum GrpcError<E> {
    Transport(tonic::transport::Error),
    Status(Status),
    Domain(E),
    Unexpected(String),
}

impl<E: Debug> Debug for GrpcError<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GrpcError::Transport(err) => write!(f, "Transport({err:?})"),
            GrpcError::Status(err) => write!(f, "Status({err:?})"),
            GrpcError::Domain(err) => write!(f, "Domain({err:?})"),
            GrpcError::Unexpected(err) => write!(f, "Unexpected({err:?})"),
        }
    }
}

impl<E: Debug> Display for GrpcError<E> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GrpcError::Transport(err) => write!(f, "gRPC transport error: {err})"),
            GrpcError::Status(err) => write!(f, "Failed gRPC request: {err})"),
            GrpcError::Domain(err) => write!(f, "gRPC request failed with {err:?}"),
            GrpcError::Unexpected(err) => write!(f, "Unexpected error {err}"),
        }
    }
}

impl<E: Debug> std::error::Error for GrpcError<E> {}

impl<E> From<tonic::transport::Error> for GrpcError<E> {
    fn from(value: tonic::transport::Error) -> Self {
        Self::Transport(value)
    }
}

impl<E> From<Status> for GrpcError<E> {
    fn from(value: Status) -> Self {
        Self::Status(value)
    }
}

impl<E> From<String> for GrpcError<E> {
    fn from(value: String) -> Self {
        Self::Unexpected(value)
    }
}

pub fn is_grpc_retriable<E>(error: &GrpcError<E>) -> bool {
    match error {
        GrpcError::Transport(_) => true,
        GrpcError::Status(status) => status.code() == tonic::Code::Unavailable,
        GrpcError::Domain(_) => false,
        GrpcError::Unexpected(_) => false,
    }
}

/// This is the implementation of the Worker Executor gRPC API
pub struct WorkerExecutorImpl<
    Ctx: WorkerCtx,
    Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static,
> {
    /// Reference to all the initialized services
    services: Svcs,
    ctx: PhantomData<Ctx>,
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static> Clone
    for WorkerExecutorImpl<Ctx, Svcs>
{
    fn clone(&self) -> Self {
        Self {
            services: self.services.clone(),
            ctx: PhantomData,
        }
    }
}

type ResponseResult<T> = Result<Response<T>, Status>;
type ResponseStream = WorkerEventStream;

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub async fn new(
        services: Svcs,
        lazy_worker_activator: Arc<LazyWorkerActivator<Ctx>>,
        port: u16,
    ) -> Result<Self, Error> {
        let worker_executor = WorkerExecutorImpl {
            services: services.clone(),
            ctx: PhantomData,
        };
        let worker_activator = Arc::new(DefaultWorkerActivator::new(services.clone()));

        lazy_worker_activator.set(worker_activator);

        let host = gethostname().to_string_lossy().to_string();

        info!(host, port, "Registering worker executor");

        let shard_assignment = worker_executor
            .shard_manager_service()
            .register(host, port)
            .await?;

        worker_executor.shard_service().register(
            shard_assignment.number_of_shards,
            &shard_assignment.shard_ids,
        );

        info!("Registered worker executor, waiting for shard assignment...");

        Ctx::on_shard_assignment_changed(&worker_executor).await?;

        Ok(worker_executor)
    }

    async fn validate_worker_status(
        &self,
        owned_worker_id: &OwnedWorkerId,
        metadata: &Option<WorkerMetadata>,
    ) -> Result<WorkerStatusRecord, GolemError> {
        let worker_status =
            Ctx::compute_latest_worker_status(self, owned_worker_id, metadata).await?;

        match &worker_status.status {
            WorkerStatus::Failed => {
                let error_and_retry_count =
                    Ctx::get_last_error_and_retry_count(self, owned_worker_id).await;
                if let Some(last_error) = error_and_retry_count {
                    Err(GolemError::PreviousInvocationFailed {
                        details: last_error.error.to_string(&last_error.stderr),
                    })
                } else {
                    Err(GolemError::PreviousInvocationFailed {
                        details: "".to_string(),
                    })
                }
            }
            WorkerStatus::Exited => Err(GolemError::PreviousInvocationExited),
            _ => {
                let error_and_retry_count =
                    Ctx::get_last_error_and_retry_count(self, owned_worker_id).await;
                debug!("Last error and retry count: {:?}", error_and_retry_count);
                if let Some(last_error) = error_and_retry_count {
                    Err(GolemError::PreviousInvocationFailed {
                        details: last_error.error.to_string(&last_error.stderr),
                    })
                } else {
                    Ok(worker_status)
                }
            }
        }
    }

    fn ensure_worker_belongs_to_this_executor(
        &self,
        worker_id: &WorkerId,
    ) -> Result<(), GolemError> {
        self.shard_service().check_worker(worker_id)
    }

    async fn create_worker_internal(
        &self,
        request: golem::workerexecutor::v1::CreateWorkerRequest,
    ) -> Result<(), GolemError> {
        let worker_id = request
            .worker_id
            .ok_or(GolemError::invalid_request("worker_id not found"))?;

        let account_id = request
            .account_id
            .ok_or(GolemError::invalid_request("account_id not found"))?
            .into();

        if let Some(limits) = request.account_limits {
            Ctx::record_last_known_limits(self, &account_id, &limits.into()).await?;
        }

        let component_version = request.component_version;
        let worker_id: WorkerId = worker_id.try_into().map_err(GolemError::invalid_request)?;
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        self.ensure_worker_belongs_to_this_executor(&worker_id)?;

        let existing_worker = self.worker_service().get(&owned_worker_id).await;
        if existing_worker.is_some() {
            return Err(GolemError::worker_already_exists(worker_id.clone()));
        }

        let args = request.args;
        let env = request
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let worker = Worker::get_or_create_suspended(
            self,
            &owned_worker_id,
            Some(args),
            Some(env),
            Some(component_version),
            None,
        )
        .await?;

        let mut subscription = self.events().subscribe();
        Worker::start_if_needed(worker.clone()).await?;
        if worker.is_loading() {
            match subscription
                .wait_for(|event| match event {
                    Event::WorkerLoaded { worker_id, result }
                        if worker_id == &owned_worker_id.worker_id =>
                    {
                        Some(result.clone())
                    }
                    _ => None,
                })
                .await
            {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e),
                Err(RecvError::Closed) => Err(GolemError::unknown("Events subscription closed")),
                Err(RecvError::Lagged(_)) => Err(GolemError::unknown(
                    "Worker executor is overloaded and could not wait for worker to load",
                )),
            }
        } else {
            Ok(())
        }
    }

    async fn complete_promise_internal(
        &self,
        request: golem::workerexecutor::v1::CompletePromiseRequest,
    ) -> Result<golem::workerexecutor::v1::CompletePromiseSuccess, GolemError> {
        let promise_id = request
            .promise_id
            .ok_or(GolemError::invalid_request("promise_id not found"))?;
        let data = request.data;

        let worker_id: WorkerId = promise_id
            .worker_id
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?
            .try_into()
            .map_err(GolemError::invalid_request)?;

        let account_id: AccountId = request
            .account_id
            .ok_or(GolemError::invalid_request("account_id not found"))?
            .into();

        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        self.ensure_worker_belongs_to_this_executor(&worker_id)?;

        let promise_id: common_model::PromiseId =
            promise_id.try_into().map_err(GolemError::invalid_request)?;
        let completed = self.promise_service().complete(promise_id, data).await?;

        let metadata = self
            .worker_service()
            .get(&owned_worker_id)
            .await
            .ok_or(GolemError::worker_not_found(worker_id.clone()))?;

        let worker_status =
            Ctx::compute_latest_worker_status(self, &owned_worker_id, &Some(metadata.clone()))
                .await?;
        let should_activate = match &worker_status.status {
            WorkerStatus::Interrupted
            | WorkerStatus::Running
            | WorkerStatus::Suspended
            | WorkerStatus::Retrying => true,
            WorkerStatus::Exited | WorkerStatus::Failed | WorkerStatus::Idle => false,
        };

        if should_activate {
            // By making sure the worker is in memory. If it was suspended because of waiting
            // for a promise, replaying that call will now not suspend as the promise has been
            // completed, and the worker will continue running.
            Worker::get_or_create_running(&self.services, &owned_worker_id, None, None, None, None)
                .await?;
        }

        let success = golem::workerexecutor::v1::CompletePromiseSuccess { completed };

        Ok(success)
    }

    async fn delete_worker_internal(&self, inner: DeleteWorkerRequest) -> Result<(), GolemError> {
        let worker_id: WorkerId = inner
            .worker_id
            .ok_or(GolemError::invalid_request("worker_id not found"))?
            .try_into()
            .map_err(GolemError::invalid_request)?;

        let account_id: AccountId = inner
            .account_id
            .ok_or(GolemError::invalid_request("account_id not found"))?
            .into();

        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        self.ensure_worker_belongs_to_this_executor(&worker_id)?;

        let metadata = self.worker_service().get(&owned_worker_id).await;
        let worker_status =
            Ctx::compute_latest_worker_status(self, &owned_worker_id, &metadata).await?;

        let should_interrupt = match &worker_status.status {
            WorkerStatus::Idle
            | WorkerStatus::Running
            | WorkerStatus::Suspended
            | WorkerStatus::Retrying => true,
            WorkerStatus::Exited | WorkerStatus::Failed | WorkerStatus::Interrupted => false,
        };

        if should_interrupt {
            let worker =
                Worker::get_or_create_suspended(self, &owned_worker_id, None, None, None, None)
                    .await?;

            if let Some(mut await_interrupted) =
                worker.set_interrupting(InterruptKind::Interrupt).await
            {
                await_interrupted.recv().await.unwrap();
            }

            worker.stop().await;
        }

        Ctx::on_worker_deleted(self, &worker_id).await?;
        self.worker_service().remove(&owned_worker_id).await;
        self.active_workers().remove(&worker_id);

        Ok(())
    }

    async fn fork_worker_internal(
        &self,
        request: ForkWorkerRequest,
    ) -> Result<ForkWorkerResponse, GolemError> {
        let account_id_proto = request
            .account_id
            .clone()
            .ok_or(GolemError::invalid_request("account_id not found"))?;

        let account_id = account_id_proto.into();

        let target_worker_id_proto = request
            .target_worker_id
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?;

        let target_worker_id: WorkerId = target_worker_id_proto
            .try_into()
            .map_err(GolemError::invalid_request)?;

        let owned_target_worker_id = OwnedWorkerId::new(&account_id, &target_worker_id);

        let source_worker_id_proto = request
            .source_worker_id
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?;

        let source_worker_id: WorkerId = source_worker_id_proto
            .try_into()
            .map_err(GolemError::invalid_request)?;

        let owned_source_worker_id = OwnedWorkerId::new(&account_id, &source_worker_id);

        self.services
            .worker_fork_service()
            .fork(
                &owned_source_worker_id,
                &owned_target_worker_id.worker_id,
                OplogIndex::from_u64(request.oplog_index_cutoff),
            )
            .await?;

        Ok(ForkWorkerResponse {
            result: Some(
                golem::workerexecutor::v1::fork_worker_response::Result::Success(
                    golem::common::Empty {},
                ),
            ),
        })
    }

    async fn interrupt_worker_internal(
        &self,
        request: golem::workerexecutor::v1::InterruptWorkerRequest,
    ) -> Result<(), GolemError> {
        let worker_id = request
            .worker_id
            .ok_or(GolemError::invalid_request("worker_id not found"))?;

        let worker_id: WorkerId = worker_id.try_into().map_err(GolemError::invalid_request)?;

        let account_id = request
            .account_id
            .ok_or(GolemError::invalid_request("account_id not found"))?;
        let account_id: AccountId = account_id.into();

        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let metadata = self.worker_service().get(&owned_worker_id).await;
        let worker_status =
            Ctx::compute_latest_worker_status(self, &owned_worker_id, &metadata).await?;

        if metadata.is_none() {
            // Worker does not exist, we still check if it is in the list active workers due to some inconsistency
            if let Some((_, worker)) = self
                .active_workers()
                .iter()
                .find(|(id, _)| *id == worker_id)
            {
                worker
                    .set_interrupting(if request.recover_immediately {
                        InterruptKind::Restart
                    } else {
                        InterruptKind::Interrupt
                    })
                    .await;
            }
        }

        match &worker_status.status {
            WorkerStatus::Exited => {
                warn!("Attempted interrupting worker which already exited")
            }
            WorkerStatus::Idle => {
                warn!("Attempted interrupting worker which is idle")
            }
            WorkerStatus::Failed => {
                warn!("Attempted interrupting worker which is failed")
            }
            WorkerStatus::Interrupted => {
                warn!("Attempted interrupting worker which is already interrupted")
            }
            WorkerStatus::Suspended => {
                debug!("Marking suspended worker as interrupted");
                let worker =
                    Worker::get_or_create_suspended(self, &owned_worker_id, None, None, None, None)
                        .await?;
                worker.set_interrupting(InterruptKind::Interrupt).await;
                // Explicitly drop from the active worker cache - this will drop websocket connections etc.
                self.active_workers().remove(&worker_id);
            }
            WorkerStatus::Retrying => {
                debug!("Marking worker scheduled to be retried as interrupted");
                let worker =
                    Worker::get_or_create_suspended(self, &owned_worker_id, None, None, None, None)
                        .await?;
                worker.set_interrupting(InterruptKind::Interrupt).await;
                // Explicitly drop from the active worker cache - this will drop websocket connections etc.
                self.active_workers().remove(&worker_id);
            }
            WorkerStatus::Running => {
                let worker =
                    Worker::get_or_create_suspended(self, &owned_worker_id, None, None, None, None)
                        .await?;
                worker
                    .set_interrupting(if request.recover_immediately {
                        InterruptKind::Restart
                    } else {
                        InterruptKind::Interrupt
                    })
                    .await;

                // Explicitly drop from the active worker cache - this will drop websocket connections etc.
                self.active_workers().remove(&worker_id);
            }
        }

        Ok(())
    }

    async fn resume_worker_internal(
        &self,
        request: golem::workerexecutor::v1::ResumeWorkerRequest,
    ) -> Result<(), GolemError> {
        let worker_id = request
            .worker_id
            .ok_or(GolemError::invalid_request("worker_id not found"))?;

        let worker_id: WorkerId = worker_id.try_into().map_err(GolemError::invalid_request)?;

        let account_id = request
            .account_id
            .ok_or(GolemError::invalid_request("account_id not found"))?;
        let account_id: AccountId = account_id.into();

        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        self.ensure_worker_belongs_to_this_executor(&worker_id)?;

        let force_resume = request.force.unwrap_or(false);

        let metadata = self.worker_service().get(&owned_worker_id).await;

        self.validate_worker_status(&owned_worker_id, &metadata)
            .await?;

        let worker_status =
            Ctx::compute_latest_worker_status(self, &owned_worker_id, &metadata).await?;

        match &worker_status.status {
            WorkerStatus::Suspended | WorkerStatus::Interrupted | WorkerStatus::Idle => {
                info!(
                    "Activating {:?} worker {worker_id} due to explicit resume request",
                    worker_status.status
                );
                let _ = Worker::get_or_create_running(
                    &self.services,
                    &owned_worker_id,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
                Ok(())
            }
            _ if force_resume => {
                info!(
                    "Force activating {:?} worker {worker_id} due to explicit resume request",
                    worker_status.status
                );
                let _ = Worker::get_or_create_running(
                    &self.services,
                    &owned_worker_id,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
                Ok(())
            }
            _ => Err(GolemError::invalid_request(format!(
                "Worker {worker_id} is not suspended, interrupted or idle",
                worker_id = worker_id
            ))),
        }
    }

    async fn invoke_and_await_worker_internal_proto<Req: GrpcInvokeRequest>(
        &self,
        request: &Req,
    ) -> Result<Vec<Val>, GolemError> {
        let result = self.invoke_and_await_worker_internal_typed(request).await?;
        let value = golem_wasm_rpc::Value::try_from(result)
            .map_err(|e| GolemError::unknown(e.to_string()))?;

        match value {
            golem_wasm_rpc::Value::Tuple(tuple) => {
                Ok(tuple.into_iter().map(|v| v.into()).collect())
            }

            golem_wasm_rpc::Value::Record(record) => {
                Ok(record.into_iter().map(|v| v.into()).collect())
            }

            v => Err(GolemError::Unknown {
                details: format!(
                    "Values retrieved after invocation is expected. Retrivee {:?}",
                    v
                )
                .to_string(),
            }),
        }
    }

    async fn invoke_and_await_worker_internal_typed<Req: GrpcInvokeRequest>(
        &self,
        request: &Req,
    ) -> Result<TypeAnnotatedValue, GolemError> {
        let full_function_name = request.name();

        let worker = self.get_or_create(request).await?;

        let idempotency_key = request
            .idempotency_key()?
            .unwrap_or(IdempotencyKey::fresh());

        let params_val = request.input();

        let function_input = params_val
            .into_iter()
            .map(|val| val.clone().try_into())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|msg| GolemError::ValueMismatch { details: msg })?;

        let values = worker
            .invoke_and_await(idempotency_key, full_function_name, function_input)
            .await?;

        Ok(values)
    }

    async fn get_or_create<Req: CanStartWorker>(
        &self,
        request: &Req,
    ) -> Result<Arc<Worker<Ctx>>, GolemError> {
        let worker = self.get_or_create_pending(request).await?;
        Worker::start_if_needed(worker.clone()).await?;
        Ok(worker)
    }

    async fn get_or_create_pending<Req: CanStartWorker>(
        &self,
        request: &Req,
    ) -> Result<Arc<Worker<Ctx>>, GolemError> {
        let target_worker_id = request.worker_id()?;

        let current_assignment = self.shard_service().current_assignment()?;

        let unspecified_name = target_worker_id.worker_name.is_none();
        let worker_id = target_worker_id.into_worker_id(
            &current_assignment.shard_ids,
            current_assignment.number_of_shards,
        );

        if unspecified_name {
            info!(
                worker_id = worker_id.to_string(),
                "Generated new unique worker id"
            );
        }

        let account_id: AccountId = request.account_id()?;
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        self.ensure_worker_belongs_to_this_executor(&worker_id)?;

        let metadata = self.worker_service().get(&owned_worker_id).await;
        self.validate_worker_status(&owned_worker_id, &metadata)
            .await?;

        if let Some(limits) = request.account_limits() {
            Ctx::record_last_known_limits(self, &account_id, &limits.into()).await?;
        }

        Worker::get_or_create_suspended(
            self,
            &owned_worker_id,
            request.args(),
            request.env(),
            None,
            request.parent(),
        )
        .await
    }

    async fn invoke_worker_internal<Req: GrpcInvokeRequest>(
        &self,
        request: &Req,
    ) -> Result<(), GolemError> {
        let full_function_name = request.name();

        let worker = self.get_or_create(request).await?;

        let idempotency_key = request
            .idempotency_key()?
            .unwrap_or(IdempotencyKey::fresh());

        let function_input = request
            .input()
            .iter()
            .map(|val| val.clone().try_into())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|msg| GolemError::ValueMismatch { details: msg })?;

        worker
            .invoke(idempotency_key, full_function_name, function_input)
            .await?;

        Ok(())
    }

    async fn revoke_shards_internal(
        &self,
        request: golem::workerexecutor::v1::RevokeShardsRequest,
    ) -> Result<(), GolemError> {
        let proto_shard_ids = request.shard_ids;

        let shard_ids = proto_shard_ids.into_iter().map(ShardId::from).collect();

        self.shard_service().revoke_shards(&shard_ids)?;

        for (worker_id, worker_details) in self.active_workers().iter() {
            if self.shard_service().check_worker(&worker_id).is_err() {
                if let Some(mut await_interrupted) = worker_details
                    .set_interrupting(InterruptKind::Restart)
                    .await
                {
                    await_interrupted.recv().await.unwrap();
                }
            }
        }

        Ok(())
    }

    async fn assign_shards_internal(
        &self,
        request: golem::workerexecutor::v1::AssignShardsRequest,
    ) -> Result<(), GolemError> {
        let proto_shard_ids = request.shard_ids;

        let shard_ids = proto_shard_ids.into_iter().map(ShardId::from).collect();

        self.shard_service().assign_shards(&shard_ids)?;
        Ctx::on_shard_assignment_changed(self).await?;

        Ok(())
    }

    async fn get_worker_metadata_internal(
        &self,
        request: golem::workerexecutor::v1::GetWorkerMetadataRequest,
    ) -> Result<golem::worker::WorkerMetadata, GolemError> {
        let worker_id = request
            .worker_id
            .ok_or(GolemError::invalid_request("worker_id not found"))?;
        let account_id = request
            .account_id
            .ok_or(GolemError::invalid_request("account_id not found"))?;

        let worker_id: WorkerId = worker_id
            .clone()
            .try_into()
            .map_err(GolemError::invalid_request)?;
        let account_id: AccountId = account_id.clone().into();

        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let metadata = self
            .worker_service()
            .get(&owned_worker_id)
            .await
            .ok_or(GolemError::worker_not_found(worker_id.clone()))?;

        let latest_status =
            Ctx::compute_latest_worker_status(self, &owned_worker_id, &Some(metadata.clone()))
                .await?;
        let last_error_and_retry_count =
            Ctx::get_last_error_and_retry_count(self, &owned_worker_id).await;

        Ok(Self::create_proto_metadata(
            metadata,
            latest_status,
            last_error_and_retry_count,
        ))
    }

    async fn get_running_workers_metadata_internal(
        &self,
        request: GetRunningWorkersMetadataRequest,
    ) -> Result<Vec<golem::worker::WorkerMetadata>, GolemError> {
        let component_id: ComponentId = request
            .component_id
            .and_then(|t| t.try_into().ok())
            .ok_or(GolemError::invalid_request("Invalid component id"))?;

        let filter: Option<WorkerFilter> = match request.filter {
            Some(f) => Some(f.try_into().map_err(GolemError::invalid_request)?),
            _ => None,
        };

        let workers = self
            .running_worker_enumeration_service()
            .get(&component_id, filter)
            .await?;

        let result: Vec<golem::worker::WorkerMetadata> = workers
            .into_iter()
            .map(|worker| {
                let status = worker.last_known_status.clone();
                Self::create_proto_metadata(worker, status, None)
            })
            .collect();

        Ok(result)
    }

    async fn get_workers_metadata_internal(
        &self,
        request: GetWorkersMetadataRequest,
    ) -> Result<(Option<Cursor>, Vec<golem::worker::WorkerMetadata>), GolemError> {
        let component_id: ComponentId = request
            .component_id
            .and_then(|t| t.try_into().ok())
            .ok_or(GolemError::invalid_request("Invalid component id"))?;

        let account_id: AccountId = request
            .account_id
            .map(|t| t.into())
            .ok_or(GolemError::invalid_request("Invalid account id"))?;

        let filter: Option<WorkerFilter> = match request.filter {
            Some(f) => Some(f.try_into().map_err(GolemError::invalid_request)?),
            _ => None,
        };

        let (new_cursor, workers) = self
            .worker_enumeration_service()
            .get(
                &account_id,
                &component_id,
                filter,
                request
                    .cursor
                    .map(|cursor| ScanCursor {
                        cursor: cursor.cursor,
                        layer: cursor.layer as usize,
                    })
                    .unwrap_or_default(),
                request.count,
                request.precise,
            )
            .instrument(info_span!("enumerate_workers"))
            .await?;

        let mut result = Vec::new();

        for worker in workers {
            let status = worker.last_known_status.clone();
            let last_error_and_retry_count =
                Ctx::get_last_error_and_retry_count(self, &worker.owned_worker_id()).await;
            let metadata = Self::create_proto_metadata(worker, status, last_error_and_retry_count);
            result.push(metadata);
        }

        Ok((
            new_cursor.map(|cursor| Cursor {
                layer: cursor.layer as u64,
                cursor: cursor.cursor,
            }),
            result,
        ))
    }

    async fn update_worker_internal(&self, request: UpdateWorkerRequest) -> Result<(), GolemError> {
        let worker_id = request
            .worker_id
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?;
        let worker_id: WorkerId = worker_id.try_into().map_err(GolemError::invalid_request)?;

        let account_id = request
            .account_id
            .clone()
            .ok_or(GolemError::invalid_request("account_id not found"))?;
        let account_id: AccountId = account_id.into();
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let metadata = self.worker_service().get(&owned_worker_id).await;
        let mut worker_status =
            Ctx::compute_latest_worker_status(self, &owned_worker_id, &metadata).await?;
        let metadata = metadata.ok_or(GolemError::worker_not_found(worker_id.clone()))?;

        if metadata.last_known_status.component_version == request.target_version {
            return Err(GolemError::invalid_request(
                "Worker is already at the target version",
            ));
        }

        let component_metadata = self
            .component_service()
            .get_metadata(
                &account_id,
                &worker_id.component_id,
                Some(metadata.last_known_status.component_version),
            )
            .await?;
        if component_metadata.component_type == ComponentType::Ephemeral {
            return Err(GolemError::invalid_request(
                "Ephemeral workers cannot be updated",
            ));
        }

        match request.mode() {
            UpdateMode::Automatic => {
                let update_description = UpdateDescription::Automatic {
                    target_version: request.target_version,
                };

                if metadata
                    .last_known_status
                    .pending_updates
                    .iter()
                    .any(|update| update.description == update_description)
                {
                    return Err(GolemError::invalid_request(
                        "The same update is already in progress",
                    ));
                }

                match &worker_status.status {
                    WorkerStatus::Exited => {
                        warn!("Attempted updating worker which already exited")
                    }
                    WorkerStatus::Interrupted
                    | WorkerStatus::Suspended
                    | WorkerStatus::Retrying
                    | WorkerStatus::Failed => {
                        // The worker is not active.
                        //
                        // We start activating it but block on a signal.
                        // This way we eliminate the race condition of activating the worker, but have
                        // time to inject the pending update oplog entry so the at the time the worker
                        // really gets activated it is going to see it and perform the update.

                        debug!("Activating worker for update",);
                        let worker = Worker::get_or_create_suspended(
                            self,
                            &owned_worker_id,
                            None,
                            None,
                            Some(worker_status.component_version),
                            None,
                        )
                        .await?;

                        debug!("Enqueuing update");
                        worker.enqueue_update(update_description.clone()).await;

                        if worker_status.status == WorkerStatus::Failed {
                            // If the worker was previously in a permanently failed state,
                            // we reset this state to Retrying, so we can fix the failure cause
                            // with an update.
                            worker_status.status = WorkerStatus::Retrying;
                        }
                        let mut deleted_regions = worker_status.deleted_regions.clone();
                        let (pending_updates, extra_deleted_regions) = worker.pending_updates();
                        deleted_regions.set_override(extra_deleted_regions);
                        worker_status.pending_updates = pending_updates;
                        worker_status.deleted_regions = deleted_regions;
                        worker.update_status(worker_status).await;

                        debug!("Resuming initialization to perform the update",);
                        Worker::start_if_needed(worker.clone()).await?;
                    }
                    WorkerStatus::Running | WorkerStatus::Idle => {
                        // If the worker is already running we need to write to its oplog the
                        // update attempt, and then interrupt it and have it immediately restarting
                        // to begin the update.
                        let worker = Worker::get_or_create_suspended(
                            self,
                            &owned_worker_id,
                            None,
                            None,
                            None,
                            None,
                        )
                        .await?;

                        worker.enqueue_update(update_description.clone()).await;

                        debug!("Enqueued update for running worker");

                        worker.set_interrupting(InterruptKind::Restart).await;

                        debug!("Interrupted running worker for update");
                    }
                }
            }

            UpdateMode::Manual => {
                if metadata.last_known_status.pending_invocations.iter().any(|invocation|
                    matches!(invocation, TimestampedWorkerInvocation { invocation: WorkerInvocation::ManualUpdate { target_version, .. }, ..} if *target_version == request.target_version)
                ) {
                    return Err(GolemError::invalid_request(
                        "The same update is already in progress",
                    ));
                }

                // For manual update we need to invoke the worker to save the custom snapshot.
                // This is in a race condition with other worker invocations, so the whole update
                // process need to be initiated through the worker's invocation queue.

                let worker =
                    Worker::get_or_create_suspended(self, &owned_worker_id, None, None, None, None)
                        .await?;
                worker.enqueue_manual_update(request.target_version).await;
            }
        }

        Ok(())
    }

    async fn connect_worker_internal(
        &self,
        request: ConnectWorkerRequest,
    ) -> ResponseResult<<Self as WorkerExecutor>::ConnectWorkerStream> {
        let worker_id: WorkerId = request
            .worker_id
            .ok_or(GolemError::invalid_request("missing worker_id"))?
            .try_into()
            .map_err(GolemError::invalid_request)?;
        let account_id: AccountId = request
            .account_id
            .ok_or(GolemError::invalid_request("missing account_id"))?
            .into();
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        self.ensure_worker_belongs_to_this_executor(&worker_id)?;

        let metadata = self.worker_service().get(&owned_worker_id).await;
        if metadata.is_some() {
            let worker_status = self
                .validate_worker_status(&owned_worker_id, &metadata)
                .await
                .map_err(|e| {
                    error!("Failed to connect to worker {worker_id}: {:?}", e);
                    Status::internal(format!("Error connecting to worker: {e}"))
                })?;

            if worker_status.status != WorkerStatus::Interrupted {
                let event_service =
                    Worker::get_or_create_suspended(self, &owned_worker_id, None, None, None, None)
                        .await?
                        .event_service();

                let receiver = event_service.receiver();

                info!("Client connected");
                record_new_grpc_api_active_stream();

                Ok(Response::new(WorkerEventStream::new(receiver)))
            } else {
                // We don't want 'connect' to resume interrupted workers
                Err(GolemError::Interrupted {
                    kind: InterruptKind::Interrupt,
                }
                .into())
            }
        } else {
            Err(GolemError::WorkerNotFound { worker_id }.into())
        }
    }

    async fn get_oplog_internal(
        &self,
        request: GetOplogRequest,
    ) -> Result<GetOplogResponse, GolemError> {
        let worker_id = request
            .worker_id
            .ok_or(GolemError::invalid_request("worker_id not found"))?;
        let worker_id: WorkerId = worker_id.try_into().map_err(GolemError::invalid_request)?;

        let account_id = request
            .account_id
            .ok_or(GolemError::invalid_request("account_id not found"))?;
        let account_id: AccountId = account_id.into();

        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        self.ensure_worker_belongs_to_this_executor(&worker_id)?;

        let chunk = match request.cursor {
            Some(cursor) => {
                get_public_oplog_chunk(
                    self.component_service(),
                    self.oplog_service(),
                    self.plugins(),
                    &owned_worker_id,
                    cursor.current_component_version,
                    OplogIndex::from_u64(cursor.next_oplog_index),
                    min(request.count as usize, 100), // TODO: configurable maximum
                )
                .await
                .map_err(GolemError::unknown)?
            }
            None => {
                let start = OplogIndex::from_u64(request.from_oplog_index);
                let initial_component_version =
                    find_component_version_at(self.oplog_service(), &owned_worker_id, start)
                        .await?;

                get_public_oplog_chunk(
                    self.component_service(),
                    self.oplog_service(),
                    self.plugins(),
                    &owned_worker_id,
                    initial_component_version,
                    start,
                    min(request.count as usize, 100), // TODO: configurable maximum
                )
                .await
                .map_err(GolemError::unknown)?
            }
        };

        let next = if chunk.entries.is_empty() {
            None
        } else {
            Some(golem::worker::OplogCursor {
                next_oplog_index: chunk.next_oplog_index.into(),
                current_component_version: chunk.current_component_version,
            })
        };

        Ok(GetOplogResponse {
            result: Some(
                golem::workerexecutor::v1::get_oplog_response::Result::Success(
                    golem::workerexecutor::v1::GetOplogSuccessResponse {
                        entries: chunk
                            .entries
                            .into_iter()
                            .map(|entry| entry.try_into())
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(GolemError::unknown)?,
                        next,
                        first_index_in_chunk: chunk.first_index_in_chunk.into(),
                        last_index: chunk.last_index.into(),
                    },
                ),
            ),
        })
    }

    async fn search_oplog_internal(
        &self,
        request: SearchOplogRequest,
    ) -> Result<SearchOplogResponse, GolemError> {
        let worker_id = request
            .worker_id
            .ok_or(GolemError::invalid_request("worker_id not found"))?;
        let worker_id: WorkerId = worker_id.try_into().map_err(GolemError::invalid_request)?;

        let account_id = request
            .account_id
            .ok_or(GolemError::invalid_request("account_id not found"))?;
        let account_id: AccountId = account_id.into();

        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        self.ensure_worker_belongs_to_this_executor(&worker_id)?;

        let chunk = match request.cursor {
            Some(cursor) => {
                search_public_oplog(
                    self.component_service(),
                    self.oplog_service(),
                    self.plugins(),
                    &owned_worker_id,
                    cursor.current_component_version,
                    OplogIndex::from_u64(cursor.next_oplog_index),
                    min(request.count as usize, 100), // TODO: configurable maximum,
                    &request.query,
                )
                .await
                .map_err(GolemError::unknown)?
            }
            None => {
                let start = OplogIndex::INITIAL;
                let initial_component_version =
                    find_component_version_at(self.oplog_service(), &owned_worker_id, start)
                        .await?;
                search_public_oplog(
                    self.component_service(),
                    self.oplog_service(),
                    self.plugins(),
                    &owned_worker_id,
                    initial_component_version,
                    start,
                    min(request.count as usize, 100), // TODO: configurable maximum,
                    &request.query,
                )
                .await
                .map_err(GolemError::unknown)?
            }
        };

        let next = if chunk.entries.is_empty() {
            None
        } else {
            Some(golem::worker::OplogCursor {
                next_oplog_index: chunk.next_oplog_index.into(),
                current_component_version: chunk.current_component_version,
            })
        };

        Ok(SearchOplogResponse {
            result: Some(
                golem::workerexecutor::v1::search_oplog_response::Result::Success(
                    golem::workerexecutor::v1::SearchOplogSuccessResponse {
                        entries: chunk
                            .entries
                            .into_iter()
                            .map(|(idx, entry)| {
                                entry.try_into().map(|entry: golem::worker::OplogEntry| {
                                    golem::worker::OplogEntryWithIndex {
                                        oplog_index: idx.into(),
                                        entry: Some(entry),
                                    }
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(GolemError::unknown)?,
                        next,
                        last_index: chunk.last_index.into(),
                    },
                ),
            ),
        })
    }

    async fn list_directory_internal(
        &self,
        request: ListDirectoryRequest,
    ) -> Result<ListDirectoryResponse, GolemError> {
        let path = ComponentFilePath::from_abs_str(&request.path)
            .map_err(|e| GolemError::invalid_request(format!("Invalid path: {}", e)))?;

        let worker = self.get_or_create(&request).await?;

        let result = worker.list_directory(path).await?;

        let response = match result {
            ListDirectoryResult::Ok(entries) => ListDirectoryResponse {
                result: Some(
                    golem::workerexecutor::v1::list_directory_response::Result::Success(
                        golem::workerexecutor::v1::ListDirectorySuccessResponse {
                            nodes: entries.into_iter().map(|entry| entry.into()).collect(),
                        },
                    ),
                ),
            },
            ListDirectoryResult::NotFound => ListDirectoryResponse {
                result: Some(
                    golem::workerexecutor::v1::list_directory_response::Result::NotFound(
                        golem::common::Empty {},
                    ),
                ),
            },
            ListDirectoryResult::NotADirectory => ListDirectoryResponse {
                result: Some(
                    golem::workerexecutor::v1::list_directory_response::Result::NotADirectory(
                        golem::common::Empty {},
                    ),
                ),
            },
        };

        Ok(response)
    }

    async fn get_file_contents_internal(
        &self,
        request: GetFileContentsRequest,
    ) -> Result<<Self as WorkerExecutor>::GetFileContentsStream, GolemError> {
        let path = ComponentFilePath::from_abs_str(&request.file_path)
            .map_err(|e| GolemError::invalid_request(format!("Invalid path: {}", e)))?;

        let worker = self.get_or_create(&request).await?;

        let result = worker.read_file(path).await?;

        let response: <Self as WorkerExecutor>::GetFileContentsStream = match result {
            ReadFileResult::NotFound => {
                let header = golem::workerexecutor::v1::GetFileContentsResponseHeader {
                    result: Some(golem::workerexecutor::v1::get_file_contents_response_header::Result::NotFound(golem::common::Empty {})),
                };
                let header_chunk = GetFileContentsResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_file_contents_response::Result::Header(
                            header,
                        ),
                    ),
                };
                Box::pin(tokio_stream::iter(vec![Ok(header_chunk)]))
            }
            ReadFileResult::NotAFile => {
                let header = golem::workerexecutor::v1::GetFileContentsResponseHeader {
                    result: Some(golem::workerexecutor::v1::get_file_contents_response_header::Result::NotAFile(golem::common::Empty {})),
                };
                let header_chunk = GetFileContentsResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_file_contents_response::Result::Header(
                            header,
                        ),
                    ),
                };
                Box::pin(tokio_stream::iter(vec![Ok(header_chunk)]))
            }
            ReadFileResult::Ok(stream) => {
                let header = golem::workerexecutor::v1::GetFileContentsResponseHeader {
                    result: Some(golem::workerexecutor::v1::get_file_contents_response_header::Result::Success(golem::common::Empty {})),
                };
                let header_chunk = GetFileContentsResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_file_contents_response::Result::Header(
                            header,
                        ),
                    ),
                };
                let header_stream = tokio_stream::iter(vec![Ok(header_chunk)]);

                let content_stream = stream
                    .map(|item| {
                        let transformed = match item {
                            Ok(data) => {
                                GetFileContentsResponse {
                                    result: Some(
                                        golem::workerexecutor::v1::get_file_contents_response::Result::Success(data.into())
                                    )
                                }
                            }
                            Err(e) => {
                                GetFileContentsResponse {
                                    result: Some(
                                        golem::workerexecutor::v1::get_file_contents_response::Result::Failure(e.into())
                                    )
                                }
                            }
                        };
                        Ok(transformed)
                    });
                Box::pin(header_stream.chain(content_stream))
            }
        };
        Ok(response)
    }

    async fn activate_plugin_internal(
        &self,
        request: ActivatePluginRequest,
    ) -> Result<(), GolemError> {
        let worker_id = request
            .worker_id
            .ok_or(GolemError::invalid_request("worker_id not found"))?;
        let worker_id: WorkerId = worker_id.try_into().map_err(GolemError::invalid_request)?;

        let account_id = request
            .account_id
            .ok_or(GolemError::invalid_request("account_id not found"))?;
        let account_id: AccountId = account_id.into();

        let plugin_installation_id = request
            .installation_id
            .ok_or(GolemError::invalid_request("installation_id not found"))?;
        let plugin_installation_id: PluginInstallationId = plugin_installation_id
            .try_into()
            .map_err(GolemError::invalid_request)?;

        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let metadata = self.worker_service().get(&owned_worker_id).await;
        let worker_status =
            Ctx::compute_latest_worker_status(self, &owned_worker_id, &metadata).await?;

        match metadata {
            Some(metadata) => {
                // Worker exists

                if worker_status
                    .active_plugins()
                    .contains(&plugin_installation_id)
                {
                    warn!("Plugin is already activated");
                    Ok(())
                } else {
                    let component_metadata = self
                        .component_service()
                        .get_metadata(
                            &account_id,
                            &worker_id.component_id,
                            Some(metadata.last_known_status.component_version),
                        )
                        .await?;

                    if component_metadata
                        .plugin_installations
                        .iter()
                        .any(|installation| installation.id == plugin_installation_id)
                    {
                        let worker = Worker::get_or_create_suspended(
                            self,
                            &owned_worker_id,
                            None,
                            None,
                            None,
                            None,
                        )
                        .await?;
                        worker.activate_plugin(plugin_installation_id).await?;
                        Ok(())
                    } else {
                        Err(GolemError::invalid_request(
                            "Plugin installation does not belong to this worker's component",
                        ))
                    }
                }
            }
            None => Err(GolemError::worker_not_found(worker_id)),
        }
    }

    async fn deactivate_plugin_internal(
        &self,
        request: DeactivatePluginRequest,
    ) -> Result<(), GolemError> {
        let worker_id = request
            .worker_id
            .ok_or(GolemError::invalid_request("worker_id not found"))?;
        let worker_id: WorkerId = worker_id.try_into().map_err(GolemError::invalid_request)?;

        let account_id = request
            .account_id
            .ok_or(GolemError::invalid_request("account_id not found"))?;
        let account_id: AccountId = account_id.into();

        let plugin_installation_id = request
            .installation_id
            .ok_or(GolemError::invalid_request("installation_id not found"))?;
        let plugin_installation_id: PluginInstallationId = plugin_installation_id
            .try_into()
            .map_err(GolemError::invalid_request)?;

        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let metadata = self.worker_service().get(&owned_worker_id).await;
        let worker_status =
            Ctx::compute_latest_worker_status(self, &owned_worker_id, &metadata).await?;

        match metadata {
            Some(metadata) => {
                // Worker exists

                if !worker_status
                    .active_plugins()
                    .contains(&plugin_installation_id)
                {
                    warn!("Plugin is already deactivated");
                    Ok(())
                } else {
                    let component_metadata = self
                        .component_service()
                        .get_metadata(
                            &account_id,
                            &worker_id.component_id,
                            Some(metadata.last_known_status.component_version),
                        )
                        .await?;

                    if component_metadata
                        .plugin_installations
                        .iter()
                        .any(|installation| installation.id == plugin_installation_id)
                    {
                        let worker = Worker::get_or_create_suspended(
                            self,
                            &owned_worker_id,
                            None,
                            None,
                            None,
                            None,
                        )
                        .await?;
                        worker.deactivate_plugin(plugin_installation_id).await?;
                        Ok(())
                    } else {
                        Err(GolemError::invalid_request(
                            "Plugin installation does not belong to this worker's component",
                        ))
                    }
                }
            }
            None => Err(GolemError::worker_not_found(worker_id)),
        }
    }

    fn create_proto_metadata(
        metadata: WorkerMetadata,
        latest_status: WorkerStatusRecord,
        last_error_and_retry_count: Option<LastError>,
    ) -> golem::worker::WorkerMetadata {
        let mut updates = Vec::new();

        for pending_invocation in &latest_status.pending_invocations {
            if let TimestampedWorkerInvocation {
                timestamp,
                invocation: WorkerInvocation::ManualUpdate { target_version },
            } = pending_invocation
            {
                updates.push(golem::worker::UpdateRecord {
                    timestamp: Some((*timestamp).into()),
                    target_version: *target_version,
                    update: Some(golem::worker::update_record::Update::Pending(
                        golem::worker::PendingUpdate {},
                    )),
                });
            }
        }
        for pending_update in &latest_status.pending_updates {
            updates.push(golem::worker::UpdateRecord {
                timestamp: Some(pending_update.timestamp.into()),
                target_version: *pending_update.description.target_version(),
                update: Some(golem::worker::update_record::Update::Pending(
                    golem::worker::PendingUpdate {},
                )),
            });
        }
        for successful_update in &latest_status.successful_updates {
            updates.push(golem::worker::UpdateRecord {
                timestamp: Some(successful_update.timestamp.into()),
                target_version: successful_update.target_version,
                update: Some(golem::worker::update_record::Update::Successful(
                    golem::worker::SuccessfulUpdate {},
                )),
            });
        }
        for failed_update in &latest_status.failed_updates {
            updates.push(golem::worker::UpdateRecord {
                timestamp: Some(failed_update.timestamp.into()),
                target_version: failed_update.target_version,
                update: Some(golem::worker::update_record::Update::Failed(
                    golem::worker::FailedUpdate {
                        details: failed_update.details.clone(),
                    },
                )),
            });
        }
        updates.sort_by_key(|record| {
            record.timestamp.as_ref().unwrap().seconds * 1_000_000_000
                + record.timestamp.as_ref().unwrap().nanos as i64
        });

        let mut owned_resources = HashMap::new();
        for (resource_id, resource) in metadata.last_known_status.owned_resources.clone() {
            owned_resources.insert(
                resource_id.0,
                ResourceMetadata {
                    created_at: Some(resource.created_at.into()),
                    indexed: resource.indexed_resource_key.map(|t| t.into()),
                },
            );
        }

        let active_plugins = metadata.last_known_status.active_plugins().clone();

        golem::worker::WorkerMetadata {
            worker_id: Some(metadata.worker_id.into()),
            args: metadata.args.clone(),
            env: HashMap::from_iter(metadata.env.iter().cloned()),
            account_id: Some(metadata.account_id.into()),
            component_version: latest_status.component_version,
            status: Into::<golem::worker::WorkerStatus>::into(latest_status.status).into(),
            retry_count: last_error_and_retry_count
                .as_ref()
                .map(|last_error| last_error.retry_count)
                .unwrap_or_default(),

            pending_invocation_count: latest_status.pending_invocations.len() as u64,
            updates,
            created_at: Some(metadata.created_at.into()),
            last_error: last_error_and_retry_count
                .map(|last_error| last_error.error.to_string(&last_error.stderr)),
            component_size: metadata.last_known_status.component_size,
            total_linear_memory_size: metadata.last_known_status.total_linear_memory_size,
            owned_resources,
            active_plugins: active_plugins.into_iter().map(|id| id.into()).collect(),
        }
    }
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static> UsesAllDeps
    for WorkerExecutorImpl<Ctx, Svcs>
{
    type Ctx = Ctx;

    fn all(&self) -> &All<Ctx> {
        self.services.all()
    }
}

#[tonic::async_trait]
impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutor for WorkerExecutorImpl<Ctx, Svcs>
{
    async fn create_worker(
        &self,
        request: Request<golem::workerexecutor::v1::CreateWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::CreateWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "create_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
            component_version = request.component_version,
            account_id = proto_account_id_string(&request.account_id),
        );

        match self
            .create_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::CreateWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::create_worker_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::CreateWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::create_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn invoke_and_await_worker(
        &self,
        request: Request<InvokeAndAwaitWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::InvokeAndAwaitWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "invoke_and_await_worker",
            worker_id = proto_target_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
            account_id = proto_account_id_string(&request.account_id),
        );

        match self.invoke_and_await_worker_internal_proto(&request).instrument(record.span.clone()).await {
            Ok(output) => {
                let result = InvokeAndAwaitWorkerSuccess { output };

                record.succeed(Ok(Response::new(
                    golem::workerexecutor::v1::InvokeAndAwaitWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::invoke_and_await_worker_response::Result::Success(result),
                        ),
                    },
                )))
            }
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InvokeAndAwaitWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::invoke_and_await_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn invoke_and_await_worker_typed(
        &self,
        request: Request<InvokeAndAwaitWorkerRequest>,
    ) -> Result<Response<InvokeAndAwaitWorkerResponseTyped>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "invoke_and_await_worker_json_typed",
            worker_id = proto_target_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
            account_id = proto_account_id_string(&request.account_id),
        );

        match self.invoke_and_await_worker_internal_typed(&request).instrument(record.span.clone()).await {
            Ok(type_annotated_value) => {
                let result = golem::workerexecutor::v1::InvokeAndAwaitWorkerSuccessTyped {
                    output: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                        type_annotated_value: Some(type_annotated_value),
                    })
                };

                record.succeed(Ok(Response::new(
                    golem::workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result: Some(
                            golem::workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Success(result),
                        ),
                    },
                )))
            }
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result: Some(
                            golem::workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn invoke_worker(
        &self,
        request: Request<golem::workerexecutor::v1::InvokeWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::InvokeWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "invoke_worker",
            worker_id = proto_target_worker_id_string(&request.worker_id),
            function = request.name,
            account_id = proto_account_id_string(&request.account_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
        );

        match self
            .invoke_worker_internal(&request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::InvokeWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::invoke_worker_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InvokeWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::invoke_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    type ConnectWorkerStream = ResponseStream;

    async fn connect_worker(
        &self,
        request: Request<golem::workerexecutor::v1::ConnectWorkerRequest>,
    ) -> ResponseResult<Self::ConnectWorkerStream> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "connect_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
            account_id = proto_account_id_string(&request.account_id)
        );

        self.connect_worker_internal(request)
            .instrument(record.span.clone())
            .await
    }

    async fn delete_worker(
        &self,
        request: Request<golem::workerexecutor::v1::DeleteWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::DeleteWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "delete_worker",
            worker_id = proto_worker_id_string(&request.worker_id)
        );

        match self
            .delete_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::DeleteWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::delete_worker_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::DeleteWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::delete_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn complete_promise(
        &self,
        request: Request<golem::workerexecutor::v1::CompletePromiseRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::CompletePromiseResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "complete_promise",
            promise_id = proto_promise_id_string(&request.promise_id)
        );

        match self
            .complete_promise_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(success) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::CompletePromiseResponse {
                    result: Some(
                        golem::workerexecutor::v1::complete_promise_response::Result::Success(
                            success,
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::CompletePromiseResponse {
                        result: Some(
                            golem::workerexecutor::v1::complete_promise_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn interrupt_worker(
        &self,
        request: Request<golem::workerexecutor::v1::InterruptWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::InterruptWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "interrupt_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        match self
            .interrupt_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::InterruptWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::interrupt_worker_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InterruptWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::interrupt_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn revoke_shards(
        &self,
        request: Request<golem::workerexecutor::v1::RevokeShardsRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::RevokeShardsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("revoke_shards",);

        match self
            .revoke_shards_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::RevokeShardsResponse {
                    result: Some(
                        golem::workerexecutor::v1::revoke_shards_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::RevokeShardsResponse {
                        result: Some(
                            golem::workerexecutor::v1::revoke_shards_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn assign_shards(
        &self,
        request: Request<golem::workerexecutor::v1::AssignShardsRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::AssignShardsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("assign_shards",);

        match self
            .assign_shards_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::AssignShardsResponse {
                    result: Some(
                        golem::workerexecutor::v1::assign_shards_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::AssignShardsResponse {
                        result: Some(
                            golem::workerexecutor::v1::assign_shards_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn get_worker_metadata(
        &self,
        request: Request<golem::workerexecutor::v1::GetWorkerMetadataRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::GetWorkerMetadataResponse>, Status> {
        let request = request.into_inner();

        let record = recorded_grpc_api_request!(
            "get_worker_metadata",
            worker_id = proto_worker_id_string(&request.worker_id)
        );

        let result = self
            .get_worker_metadata_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(result) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::GetWorkerMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_worker_metadata_response::Result::Success(
                            result,
                        ),
                    ),
                },
            ))),
            Err(err @ GolemError::WorkerNotFound { .. }) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::GetWorkerMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_worker_metadata_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::GetWorkerMetadataResponse {
                        result: Some(
                            golem::workerexecutor::v1::get_worker_metadata_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn resume_worker(
        &self,
        request: Request<golem::workerexecutor::v1::ResumeWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::ResumeWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "resume_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        match self
            .resume_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::ResumeWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::resume_worker_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::ResumeWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::resume_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn get_running_workers_metadata(
        &self,
        request: Request<GetRunningWorkersMetadataRequest>,
    ) -> Result<Response<GetRunningWorkersMetadataResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_running_workers_metadata",
            component_id = proto_component_id_string(&request.component_id),
        );

        let result = self
            .get_running_workers_metadata_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(workers) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::GetRunningWorkersMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_running_workers_metadata_response::Result::Success(
                            golem::workerexecutor::v1::GetRunningWorkersMetadataSuccessResponse {
                                workers
                            }
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    GetRunningWorkersMetadataResponse {
                        result: Some(
                            golem::workerexecutor::v1::get_running_workers_metadata_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &err,
            ),
        }
    }

    async fn get_workers_metadata(
        &self,
        request: Request<GetWorkersMetadataRequest>,
    ) -> Result<Response<GetWorkersMetadataResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_workers_metadata",
            component_id = proto_component_id_string(&request.component_id),
        );

        let result = self
            .get_workers_metadata_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok((cursor, workers)) => {
                record.succeed(Ok(Response::new(GetWorkersMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_workers_metadata_response::Result::Success(
                            golem::workerexecutor::v1::GetWorkersMetadataSuccessResponse {
                                workers,
                                cursor,
                            },
                        ),
                    ),
                })))
            }
            Err(err) => record.fail(
                Ok(Response::new(GetWorkersMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_workers_metadata_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn update_worker(
        &self,
        request: Request<UpdateWorkerRequest>,
    ) -> Result<Response<UpdateWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "update_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
            target_version = request.target_version,
        );

        match self
            .update_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(UpdateWorkerResponse {
                result: Some(
                    golem::workerexecutor::v1::update_worker_response::Result::Success(
                        golem::common::Empty {},
                    ),
                ),
            }))),
            Err(err) => record.fail(
                Ok(Response::new(UpdateWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::update_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn get_oplog(
        &self,
        request: Request<GetOplogRequest>,
    ) -> Result<Response<GetOplogResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_oplog",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let result = self
            .get_oplog_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(response) => record.succeed(Ok(Response::new(response))),
            Err(err) => record.fail(
                Ok(Response::new(GetOplogResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_oplog_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn search_oplog(
        &self,
        request: Request<SearchOplogRequest>,
    ) -> Result<Response<SearchOplogResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "search_oplog",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let result = self
            .search_oplog_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(response) => record.succeed(Ok(Response::new(response))),
            Err(err) => record.fail(
                Ok(Response::new(SearchOplogResponse {
                    result: Some(
                        golem::workerexecutor::v1::search_oplog_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn fork_worker(
        &self,
        request: Request<ForkWorkerRequest>,
    ) -> Result<Response<ForkWorkerResponse>, Status> {
        let request = request.into_inner();

        let record = recorded_grpc_api_request!(
            "fork_worker",
            source_worker_id = proto_worker_id_string(&request.source_worker_id),
            target_worker_id = proto_worker_id_string(&request.target_worker_id),
        );

        let result = self
            .fork_worker_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(_) => record.succeed(Ok(Response::new(ForkWorkerResponse {
                result: Some(
                    golem::workerexecutor::v1::fork_worker_response::Result::Success(
                        golem::common::Empty {},
                    ),
                ),
            }))),
            Err(err) => record.fail(
                Ok(Response::new(ForkWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::fork_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn list_directory(
        &self,
        request: Request<ListDirectoryRequest>,
    ) -> ResponseResult<ListDirectoryResponse> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "list_directory",
            worker_id = proto_target_worker_id_string(&request.worker_id),
            path = request.path,
        );

        let result = self
            .list_directory_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(response) => record.succeed(Ok(Response::new(response))),
            Err(err) => record.fail(
                Ok(Response::new(ListDirectoryResponse {
                    result: Some(
                        golem::workerexecutor::v1::list_directory_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    type GetFileContentsStream =
        Pin<Box<dyn Stream<Item = Result<GetFileContentsResponse, Status>> + Send + 'static>>;

    async fn get_file_contents(
        &self,
        request: Request<GetFileContentsRequest>,
    ) -> ResponseResult<Self::GetFileContentsStream> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_file_contents",
            worker_id = proto_target_worker_id_string(&request.worker_id),
            path = request.file_path,
        );

        let result = self
            .get_file_contents_internal(request)
            .instrument(record.span.clone())
            .await;

        let stream: Self::GetFileContentsStream = match result {
            Ok(stream) => record.succeed(stream),
            Err(err) => {
                let res = GetFileContentsResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_file_contents_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                };

                let err_stream: Self::GetFileContentsStream =
                    Box::pin(tokio_stream::iter(vec![Ok(res)]));

                record.fail(err_stream, &err)
            }
        };
        Ok(Response::new(stream))
    }

    async fn activate_plugin(
        &self,
        request: Request<ActivatePluginRequest>,
    ) -> Result<Response<ActivatePluginResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "activate_plugin",
            worker_id = proto_worker_id_string(&request.worker_id),
            plugin_installation_id = proto_plugin_installation_id_string(&request.installation_id)
        );

        let result = self
            .activate_plugin_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(_) => record.succeed(Ok(Response::new(ActivatePluginResponse {
                result: Some(
                    golem::workerexecutor::v1::activate_plugin_response::Result::Success(
                        golem::common::Empty {},
                    ),
                ),
            }))),
            Err(err) => record.fail(
                Ok(Response::new(ActivatePluginResponse {
                    result: Some(
                        golem::workerexecutor::v1::activate_plugin_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn deactivate_plugin(
        &self,
        request: Request<DeactivatePluginRequest>,
    ) -> Result<Response<DeactivatePluginResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "deactivate_plugin",
            worker_id = proto_worker_id_string(&request.worker_id),
            plugin_installation_id = proto_plugin_installation_id_string(&request.installation_id)
        );

        let result = self
            .deactivate_plugin_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(_) => record.succeed(Ok(Response::new(DeactivatePluginResponse {
                result: Some(
                    golem::workerexecutor::v1::deactivate_plugin_response::Result::Success(
                        golem::common::Empty {},
                    ),
                ),
            }))),
            Err(err) => record.fail(
                Ok(Response::new(DeactivatePluginResponse {
                    result: Some(
                        golem::workerexecutor::v1::deactivate_plugin_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }
}

trait CanStartWorker {
    fn account_id(&self) -> Result<AccountId, GolemError>;
    fn account_limits(&self) -> Option<GrpcResourceLimits>;
    fn worker_id(&self) -> Result<TargetWorkerId, GolemError>;
    fn args(&self) -> Option<Vec<String>>;
    fn env(&self) -> Option<Vec<(String, String)>>;
    fn parent(&self) -> Option<WorkerId>;
}

trait GrpcInvokeRequest: CanStartWorker {
    fn input(&self) -> Vec<Val>;
    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError>;
    fn name(&self) -> String;
}

impl CanStartWorker for golem::workerexecutor::v1::ListDirectoryRequest {
    fn account_id(&self) -> Result<AccountId, GolemError> {
        Ok(self
            .account_id
            .clone()
            .ok_or(GolemError::invalid_request("account_id not found"))?
            .into())
    }

    fn account_limits(&self) -> Option<GrpcResourceLimits> {
        self.account_limits
    }

    fn worker_id(&self) -> Result<common_model::TargetWorkerId, GolemError> {
        self.worker_id
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?
            .try_into()
            .map_err(GolemError::invalid_request)
    }

    fn args(&self) -> Option<Vec<String>> {
        None
    }

    fn env(&self) -> Option<Vec<(String, String)>> {
        None
    }

    fn parent(&self) -> Option<WorkerId> {
        None
    }
}

impl CanStartWorker for golem::workerexecutor::v1::GetFileContentsRequest {
    fn account_id(&self) -> Result<AccountId, GolemError> {
        Ok(self
            .account_id
            .clone()
            .ok_or(GolemError::invalid_request("account_id not found"))?
            .into())
    }

    fn account_limits(&self) -> Option<GrpcResourceLimits> {
        self.account_limits
    }

    fn worker_id(&self) -> Result<common_model::TargetWorkerId, GolemError> {
        self.worker_id
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?
            .try_into()
            .map_err(GolemError::invalid_request)
    }

    fn args(&self) -> Option<Vec<String>> {
        None
    }

    fn env(&self) -> Option<Vec<(String, String)>> {
        None
    }

    fn parent(&self) -> Option<WorkerId> {
        None
    }
}

impl CanStartWorker for golem::workerexecutor::v1::InvokeWorkerRequest {
    fn account_id(&self) -> Result<AccountId, GolemError> {
        Ok(self
            .account_id
            .clone()
            .ok_or(GolemError::invalid_request("account_id not found"))?
            .into())
    }

    fn account_limits(&self) -> Option<GrpcResourceLimits> {
        self.account_limits
    }

    fn worker_id(&self) -> Result<common_model::TargetWorkerId, GolemError> {
        self.worker_id
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?
            .try_into()
            .map_err(GolemError::invalid_request)
    }

    fn args(&self) -> Option<Vec<String>> {
        self.context.as_ref().map(|ctx| ctx.args.clone())
    }

    fn env(&self) -> Option<Vec<(String, String)>> {
        self.context
            .as_ref()
            .map(|ctx| ctx.env.clone().into_iter().collect::<Vec<_>>())
    }

    fn parent(&self) -> Option<WorkerId> {
        self.context.as_ref().and_then(|ctx| {
            ctx.parent
                .as_ref()
                .and_then(|worker_id| worker_id.clone().try_into().ok())
        })
    }
}

impl GrpcInvokeRequest for golem::workerexecutor::v1::InvokeWorkerRequest {
    fn input(&self) -> Vec<Val> {
        self.input.clone()
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError> {
        Ok(self.idempotency_key.clone().map(IdempotencyKey::from))
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

impl CanStartWorker for golem::workerexecutor::v1::InvokeAndAwaitWorkerRequest {
    fn account_id(&self) -> Result<AccountId, GolemError> {
        Ok(self
            .account_id
            .clone()
            .ok_or(GolemError::invalid_request("account_id not found"))?
            .into())
    }

    fn account_limits(&self) -> Option<GrpcResourceLimits> {
        self.account_limits
    }

    fn worker_id(&self) -> Result<common_model::TargetWorkerId, GolemError> {
        self.worker_id
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?
            .try_into()
            .map_err(GolemError::invalid_request)
    }

    fn args(&self) -> Option<Vec<String>> {
        self.context.as_ref().map(|ctx| ctx.args.clone())
    }

    fn env(&self) -> Option<Vec<(String, String)>> {
        self.context
            .as_ref()
            .map(|ctx| ctx.env.clone().into_iter().collect::<Vec<_>>())
    }

    fn parent(&self) -> Option<WorkerId> {
        self.context.as_ref().and_then(|ctx| {
            ctx.parent
                .as_ref()
                .and_then(|worker_id| worker_id.clone().try_into().ok())
        })
    }
}

impl GrpcInvokeRequest for golem::workerexecutor::v1::InvokeAndAwaitWorkerRequest {
    fn input(&self) -> Vec<Val> {
        self.input.clone()
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError> {
        Ok(self.idempotency_key.clone().map(IdempotencyKey::from))
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

pub fn authorised_grpc_request<T>(request: T, access_token: &Uuid) -> Request<T> {
    let mut req = Request::new(request);
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {}", access_token).parse().unwrap(),
    );
    req
}

pub struct WorkerEventStream {
    inner: Pin<Box<dyn Stream<Item = Result<WorkerEvent, BroadcastStreamRecvError>> + Send>>,
}

impl WorkerEventStream {
    pub fn new(receiver: WorkerEventReceiver) -> Self {
        WorkerEventStream {
            inner: Box::pin(receiver.to_stream()),
        }
    }
}

impl Drop for WorkerEventStream {
    fn drop(&mut self) {
        info!("Client disconnected");
    }
}

impl Stream for WorkerEventStream {
    type Item = Result<golem::worker::LogEvent, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let WorkerEventStream { inner } = self.get_mut();
        match inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => match &event {
                WorkerEvent::Close => Poll::Ready(None),
                WorkerEvent::StdOut { .. } => Poll::Ready(Some(Ok(event.try_into().unwrap()))),
                WorkerEvent::StdErr { .. } => Poll::Ready(Some(Ok(event.try_into().unwrap()))),
                WorkerEvent::Log { .. } => Poll::Ready(Some(Ok(event.try_into().unwrap()))),
                WorkerEvent::InvocationStart { .. } => {
                    Poll::Ready(Some(Ok(event.try_into().unwrap())))
                }
                WorkerEvent::InvocationFinished { .. } => {
                    Poll::Ready(Some(Ok(event.try_into().unwrap())))
                }
            },
            Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(n)))) => Poll::Ready(Some(Err(
                Status::data_loss(format!("Lagged by {} events", n)),
            ))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
