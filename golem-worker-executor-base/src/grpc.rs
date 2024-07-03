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

use gethostname::gethostname;
use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;

use std::sync::Arc;

use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::common::ResourceLimits as GrpcResourceLimits;
use golem_api_grpc::proto::golem::worker::{Cursor, UpdateMode};
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_server::WorkerExecutor;
use golem_api_grpc::proto::golem::workerexecutor::{
    ConnectWorkerRequest, DeleteWorkerRequest, GetRunningWorkersMetadataRequest,
    GetRunningWorkersMetadataResponse, GetWorkersMetadataRequest, GetWorkersMetadataResponse,
    UpdateWorkerRequest, UpdateWorkerResponse,
};
use golem_common::model as common_model;
use golem_common::model::oplog::UpdateDescription;
use golem_common::model::{
    AccountId, CallingConvention, ComponentId, IdempotencyKey, OwnedWorkerId, PromiseId,
    ScanCursor, ShardId, TimestampedWorkerInvocation, WorkerFilter, WorkerId, WorkerInvocation,
    WorkerMetadata, WorkerStatus, WorkerStatusRecord,
};
use golem_wasm_rpc::protobuf::Val;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info, warn, Instrument};
use uuid::Uuid;
use wasmtime::Error;

use crate::error::*;
use crate::metrics::grpc::{record_closed_grpc_active_stream, record_new_grpc_active_stream};
use crate::model::{InterruptKind, LastError};
use crate::recorded_grpc_request;
use crate::services::worker_activator::{DefaultWorkerActivator, LazyWorkerActivator};
use crate::services::worker_event::LogLevel;
use crate::services::{
    worker_event, All, HasActiveWorkers, HasAll, HasPromiseService,
    HasRunningWorkerEnumerationService, HasShardManagerService, HasShardService,
    HasWorkerEnumerationService, HasWorkerService, UsesAllDeps,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;

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
type ResponseStream = ReceiverStream<Result<golem::worker::LogEvent, Status>>;

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub async fn new(
        services: Svcs,
        lazy_worker_activator: Arc<LazyWorkerActivator>,
        port: u16,
    ) -> Result<Self, Error> {
        let worker_executor = WorkerExecutorImpl {
            services: services.clone(),
            ctx: PhantomData,
        };
        let worker_activator = Arc::new(DefaultWorkerActivator::new(services));
        lazy_worker_activator.set(worker_activator);

        let host = gethostname().to_string_lossy().to_string();

        info!("Registering worker executor as {}:{}", host, port);

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
                        details: format!("{}", last_error.error),
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
                        details: format!("{}", last_error.error),
                    })
                } else {
                    Ok(worker_status)
                }
            }
        }
    }

    fn ensure_worker_belongs_to_this_executor(
        &self,
        worker_id: &common_model::WorkerId,
    ) -> Result<(), GolemError> {
        self.shard_service().check_worker(worker_id)
    }

    async fn create_worker_internal(
        &self,
        request: golem::workerexecutor::CreateWorkerRequest,
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
        Worker::start_if_needed(worker.clone()).await?;

        Ok(())
    }

    async fn complete_promise_internal(
        &self,
        request: golem::workerexecutor::CompletePromiseRequest,
    ) -> Result<golem::workerexecutor::CompletePromiseSuccess, GolemError> {
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

        let success = golem::workerexecutor::CompletePromiseSuccess { completed };

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

    async fn interrupt_worker_internal(
        &self,
        request: golem::workerexecutor::InterruptWorkerRequest,
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
                .enum_workers()
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
        request: golem::workerexecutor::ResumeWorkerRequest,
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

        let metadata = self.worker_service().get(&owned_worker_id).await;
        self.validate_worker_status(&owned_worker_id, &metadata)
            .await?;

        let worker_status =
            Ctx::compute_latest_worker_status(self, &owned_worker_id, &metadata).await?;

        match &worker_status.status {
            WorkerStatus::Suspended | WorkerStatus::Interrupted => {
                info!("Activating ${worker_status:?} worker {worker_id} due to explicit resume request");
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
                "Worker {worker_id} is not suspended or interrupted",
                worker_id = worker_id
            ))),
        }
    }

    async fn invoke_and_await_worker_internal(
        &self,
        request: &golem::workerexecutor::InvokeAndAwaitWorkerRequest,
    ) -> Result<golem::workerexecutor::InvokeAndAwaitWorkerSuccess, GolemError> {
        let full_function_name = request.name();

        let proto_function_input: Vec<Val> = request.input();
        let function_input = proto_function_input
            .iter()
            .map(|val| val.clone().try_into())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|msg| GolemError::ValueMismatch { details: msg })?;

        let calling_convention = request.calling_convention();
        let worker = self.get_or_create(request).await?;
        let idempotency_key = request
            .idempotency_key()?
            .unwrap_or(IdempotencyKey::fresh());

        let values = worker
            .invoke_and_await(
                idempotency_key,
                calling_convention.into(),
                full_function_name,
                function_input,
            )
            .await?;
        let output = values.into_iter().map(|val| val.into()).collect();
        Ok(golem::workerexecutor::InvokeAndAwaitWorkerSuccess { output })
    }

    async fn get_or_create<Req: GrpcInvokeRequest>(
        &self,
        request: &Req,
    ) -> Result<Arc<Worker<Ctx>>, GolemError> {
        let worker = self.get_or_create_pending(request).await?;
        Worker::start_if_needed(worker.clone()).await?;
        Ok(worker)
    }

    async fn get_or_create_pending<Req: GrpcInvokeRequest>(
        &self,
        request: &Req,
    ) -> Result<Arc<Worker<Ctx>>, GolemError> {
        let worker_id = request.worker_id()?;
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

        let proto_function_input: Vec<Val> = request.input();
        let function_input = proto_function_input
            .iter()
            .map(|val| val.clone().try_into())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|msg| GolemError::ValueMismatch { details: msg })?;

        let calling_convention = request.calling_convention();

        let worker = self.get_or_create(request).await?;
        let idempotency_key = request
            .idempotency_key()?
            .unwrap_or(IdempotencyKey::fresh());

        worker
            .invoke(
                idempotency_key,
                calling_convention,
                full_function_name,
                function_input,
            )
            .await?;

        Ok(())
    }

    async fn revoke_shards_internal(
        &self,
        request: golem::workerexecutor::RevokeShardsRequest,
    ) -> Result<(), GolemError> {
        let proto_shard_ids = request.shard_ids;

        let shard_ids = proto_shard_ids.into_iter().map(ShardId::from).collect();

        self.shard_service().revoke_shards(&shard_ids);

        let workers = self.active_workers().enum_workers();

        for (worker_id, worker_details) in workers {
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
        request: golem::workerexecutor::AssignShardsRequest,
    ) -> Result<(), GolemError> {
        let proto_shard_ids = request.shard_ids;

        let shard_ids = proto_shard_ids.into_iter().map(ShardId::from).collect();

        self.shard_service().assign_shards(&shard_ids);
        Ctx::on_shard_assignment_changed(self).await?;

        Ok(())
    }

    async fn get_worker_metadata_internal(
        &self,
        request: golem::workerexecutor::GetWorkerMetadataRequest,
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
            .await?;

        let result: Vec<golem::worker::WorkerMetadata> = workers
            .into_iter()
            .map(|worker| {
                let status = worker.last_known_status.clone();
                Self::create_proto_metadata(worker, status, None)
            })
            .collect();

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

                let mut receiver = event_service.receiver();

                info!("Client connected");
                record_new_grpc_active_stream();

                // spawn and channel are required if you want handle "disconnect" functionality
                // the `out_stream` will not be polled after client disconnect
                let (tx, rx) = mpsc::channel(128);

                tokio::spawn(
                    async move {
                        while let Ok(item) = receiver.recv().await {
                            match item {
                                worker_event::WorkerEvent::Close => {
                                    break;
                                }
                                worker_event::WorkerEvent::StdOut(line) => {
                                    match tx
                                        .send(Result::<_, Status>::Ok(golem::worker::LogEvent {
                                            event: Some(golem::worker::log_event::Event::Stdout(
                                                golem::worker::StdOutLog {
                                                    message: String::from_utf8(line).unwrap(),
                                                },
                                            )),
                                        }))
                                        .await
                                    {
                                        Ok(_) => {
                                            // item (server response) was queued to be send to client
                                        }
                                        Err(_item) => {
                                            // output_stream was build from rx and both are dropped
                                            break;
                                        }
                                    }
                                }
                                worker_event::WorkerEvent::StdErr(line) => {
                                    match tx
                                        .send(Result::<_, Status>::Ok(golem::worker::LogEvent {
                                            event: Some(golem::worker::log_event::Event::Stderr(
                                                golem::worker::StdErrLog {
                                                    message: String::from_utf8(line).unwrap(),
                                                },
                                            )),
                                        }))
                                        .await
                                    {
                                        Ok(_) => {
                                            // item (server response) was queued to be send to client
                                        }
                                        Err(_item) => {
                                            // output_stream was build from rx and both are dropped
                                            break;
                                        }
                                    }
                                }
                                worker_event::WorkerEvent::Log {
                                    level,
                                    context,
                                    message,
                                } => match tx
                                    .send(Result::<_, Status>::Ok(golem::worker::LogEvent {
                                        event: Some(golem::worker::log_event::Event::Log(
                                            golem::worker::Log {
                                                level: match level {
                                                    LogLevel::Trace => {
                                                        golem::worker::Level::Trace.into()
                                                    }
                                                    LogLevel::Debug => {
                                                        golem::worker::Level::Debug.into()
                                                    }
                                                    LogLevel::Info => {
                                                        golem::worker::Level::Info.into()
                                                    }
                                                    LogLevel::Warn => {
                                                        golem::worker::Level::Warn.into()
                                                    }
                                                    LogLevel::Error => {
                                                        golem::worker::Level::Error.into()
                                                    }
                                                    LogLevel::Critical => {
                                                        golem::worker::Level::Critical.into()
                                                    }
                                                },
                                                context,
                                                message,
                                            },
                                        )),
                                    }))
                                    .await
                                {
                                    Ok(_) => {
                                        // item (server response) was queued to be send to client
                                    }
                                    Err(_item) => {
                                        // output_stream was build from rx and both are dropped
                                        break;
                                    }
                                },
                            }
                        }

                        record_closed_grpc_active_stream();
                        info!("Client disconnected");
                    }
                    .in_current_span(),
                );

                let output_stream = ReceiverStream::new(rx);
                Ok(Response::new(output_stream))
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
            last_error: last_error_and_retry_count.map(|last_error| last_error.error.to_string()),
            component_size: metadata.last_known_status.component_size,
            total_linear_memory_size: metadata.last_known_status.total_linear_memory_size,
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

fn proto_component_id_string(
    component_id: &Option<golem_api_grpc::proto::golem::component::ComponentId>,
) -> Option<String> {
    component_id
        .clone()
        .and_then(|v| TryInto::<ComponentId>::try_into(v).ok())
        .map(|v| v.to_string())
}

fn proto_worker_id_string(
    worker_id: &Option<golem_api_grpc::proto::golem::worker::WorkerId>,
) -> Option<String> {
    worker_id
        .clone()
        .and_then(|v| TryInto::<WorkerId>::try_into(v).ok())
        .map(|v| v.to_string())
}

fn proto_idempotency_key_string(
    idempotency_key: &Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
) -> Option<String> {
    idempotency_key
        .clone()
        .map(|v| Into::<IdempotencyKey>::into(v).to_string())
}

fn proto_account_id_string(
    account_id: &Option<golem_api_grpc::proto::golem::common::AccountId>,
) -> Option<String> {
    account_id
        .clone()
        .map(|v| Into::<AccountId>::into(v).to_string())
}

fn proto_promise_id_string(
    promise_id: &Option<golem_api_grpc::proto::golem::worker::PromiseId>,
) -> Option<String> {
    promise_id
        .clone()
        .and_then(|v| TryInto::<PromiseId>::try_into(v).ok())
        .map(|v| v.to_string())
}

#[tonic::async_trait]
impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutor for WorkerExecutorImpl<Ctx, Svcs>
{
    async fn create_worker(
        &self,
        request: Request<golem::workerexecutor::CreateWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::CreateWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_request!(
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
                golem::workerexecutor::CreateWorkerResponse {
                    result: Some(
                        golem::workerexecutor::create_worker_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(golem::workerexecutor::CreateWorkerResponse {
                    result: Some(
                        golem::workerexecutor::create_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn invoke_and_await_worker(
        &self,
        request: Request<golem::workerexecutor::InvokeAndAwaitWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::InvokeAndAwaitWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_request!(
            "invoke_and_await_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
            calling_convention = request.calling_convention,
            account_id = proto_account_id_string(&request.account_id),
        );

        match self.invoke_and_await_worker_internal(&request).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(Ok(Response::new(
                golem::workerexecutor::InvokeAndAwaitWorkerResponse {
                    result: Some(
                        golem::workerexecutor::invoke_and_await_worker_response::Result::Success(result),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::InvokeAndAwaitWorkerResponse {
                        result: Some(
                            golem::workerexecutor::invoke_and_await_worker_response::Result::Failure(
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
        request: Request<golem::workerexecutor::InvokeWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::InvokeWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_request!(
            "invoke_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
            function = request.name,
            account_id = proto_account_id_string(&request.account_id)
        );

        match self
            .invoke_worker_internal(&request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::InvokeWorkerResponse {
                    result: Some(
                        golem::workerexecutor::invoke_worker_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(golem::workerexecutor::InvokeWorkerResponse {
                    result: Some(
                        golem::workerexecutor::invoke_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    type ConnectWorkerStream = ResponseStream;

    async fn connect_worker(
        &self,
        request: Request<golem::workerexecutor::ConnectWorkerRequest>,
    ) -> ResponseResult<Self::ConnectWorkerStream> {
        let request = request.into_inner();
        let record = recorded_grpc_request!(
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
        request: Request<golem::workerexecutor::DeleteWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::DeleteWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_request!(
            "delete_worker",
            worker_id = proto_worker_id_string(&request.worker_id)
        );

        match self
            .delete_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::DeleteWorkerResponse {
                    result: Some(
                        golem::workerexecutor::delete_worker_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(golem::workerexecutor::DeleteWorkerResponse {
                    result: Some(
                        golem::workerexecutor::delete_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn complete_promise(
        &self,
        request: Request<golem::workerexecutor::CompletePromiseRequest>,
    ) -> Result<Response<golem::workerexecutor::CompletePromiseResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_request!(
            "complete_promise",
            promise_id = proto_promise_id_string(&request.promise_id)
        );

        match self
            .complete_promise_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(success) => record.succeed(Ok(Response::new(
                golem::workerexecutor::CompletePromiseResponse {
                    result: Some(
                        golem::workerexecutor::complete_promise_response::Result::Success(success),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::CompletePromiseResponse {
                        result: Some(
                            golem::workerexecutor::complete_promise_response::Result::Failure(
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
        request: Request<golem::workerexecutor::InterruptWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::InterruptWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_request!(
            "interrupt_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        match self
            .interrupt_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::InterruptWorkerResponse {
                    result: Some(
                        golem::workerexecutor::interrupt_worker_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::InterruptWorkerResponse {
                        result: Some(
                            golem::workerexecutor::interrupt_worker_response::Result::Failure(
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
        request: Request<golem::workerexecutor::RevokeShardsRequest>,
    ) -> Result<Response<golem::workerexecutor::RevokeShardsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_request!("revoke_shards",);

        match self
            .revoke_shards_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::RevokeShardsResponse {
                    result: Some(
                        golem::workerexecutor::revoke_shards_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(golem::workerexecutor::RevokeShardsResponse {
                    result: Some(
                        golem::workerexecutor::revoke_shards_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn assign_shards(
        &self,
        request: Request<golem::workerexecutor::AssignShardsRequest>,
    ) -> Result<Response<golem::workerexecutor::AssignShardsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_request!("assign_shards",);

        match self
            .assign_shards_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::AssignShardsResponse {
                    result: Some(
                        golem::workerexecutor::assign_shards_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(golem::workerexecutor::AssignShardsResponse {
                    result: Some(
                        golem::workerexecutor::assign_shards_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn get_worker_metadata(
        &self,
        request: Request<golem::workerexecutor::GetWorkerMetadataRequest>,
    ) -> Result<Response<golem::workerexecutor::GetWorkerMetadataResponse>, Status> {
        let request = request.into_inner();

        let record = recorded_grpc_request!(
            "get_worker_metadata",
            worker_id = proto_worker_id_string(&request.worker_id)
        );

        let result = self
            .get_worker_metadata_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(result) => record.succeed(Ok(Response::new(
                golem::workerexecutor::GetWorkerMetadataResponse {
                    result: Some(
                        golem::workerexecutor::get_worker_metadata_response::Result::Success(
                            result,
                        ),
                    ),
                },
            ))),
            Err(err @ GolemError::WorkerNotFound { .. }) => record.succeed(Ok(Response::new(
                golem::workerexecutor::GetWorkerMetadataResponse {
                    result: Some(
                        golem::workerexecutor::get_worker_metadata_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::GetWorkerMetadataResponse {
                        result: Some(
                            golem::workerexecutor::get_worker_metadata_response::Result::Failure(
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
        request: Request<golem::workerexecutor::ResumeWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::ResumeWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_request!(
            "resume_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        match self
            .resume_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::ResumeWorkerResponse {
                    result: Some(
                        golem::workerexecutor::resume_worker_response::Result::Success(
                            golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(err) => record.fail(
                Ok(Response::new(golem::workerexecutor::ResumeWorkerResponse {
                    result: Some(
                        golem::workerexecutor::resume_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }

    async fn get_running_workers_metadata(
        &self,
        request: Request<GetRunningWorkersMetadataRequest>,
    ) -> Result<Response<GetRunningWorkersMetadataResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_request!(
            "get_running_workers_metadata",
            component_id = proto_component_id_string(&request.component_id),
        );

        let result = self
            .get_running_workers_metadata_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(workers) => record.succeed(Ok(Response::new(
                golem::workerexecutor::GetRunningWorkersMetadataResponse {
                    result: Some(
                        golem::workerexecutor::get_running_workers_metadata_response::Result::Success(
                            golem::workerexecutor::GetRunningWorkersMetadataSuccessResponse {
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
                            golem::workerexecutor::get_running_workers_metadata_response::Result::Failure(
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
        let record = recorded_grpc_request!(
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
                        golem::workerexecutor::get_workers_metadata_response::Result::Success(
                            golem::workerexecutor::GetWorkersMetadataSuccessResponse {
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
                        golem::workerexecutor::get_workers_metadata_response::Result::Failure(
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
        let record = recorded_grpc_request!(
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
                    golem::workerexecutor::update_worker_response::Result::Success(
                        golem::common::Empty {},
                    ),
                ),
            }))),
            Err(err) => record.fail(
                Ok(Response::new(UpdateWorkerResponse {
                    result: Some(
                        golem::workerexecutor::update_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &err,
            ),
        }
    }
}

trait GrpcInvokeRequest {
    fn account_id(&self) -> Result<AccountId, GolemError>;
    fn account_limits(&self) -> Option<GrpcResourceLimits>;
    fn calling_convention(&self) -> CallingConvention;
    fn input(&self) -> Vec<Val>;
    fn worker_id(&self) -> Result<WorkerId, GolemError>;
    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError>;
    fn name(&self) -> String;
    fn args(&self) -> Option<Vec<String>>;
    fn env(&self) -> Option<Vec<(String, String)>>;
    fn parent(&self) -> Option<WorkerId>;
}

impl GrpcInvokeRequest for golem::workerexecutor::InvokeWorkerRequest {
    fn account_id(&self) -> Result<AccountId, GolemError> {
        Ok(self
            .account_id
            .clone()
            .ok_or(GolemError::invalid_request("account_id not found"))?
            .into())
    }

    fn account_limits(&self) -> Option<GrpcResourceLimits> {
        self.account_limits.clone()
    }

    fn calling_convention(&self) -> CallingConvention {
        CallingConvention::Component
    }

    fn input(&self) -> Vec<Val> {
        self.input.clone()
    }

    fn worker_id(&self) -> Result<common_model::WorkerId, GolemError> {
        self.worker_id
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?
            .try_into()
            .map_err(GolemError::invalid_request)
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError> {
        Ok(None)
    }

    fn name(&self) -> String {
        self.name.clone()
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

impl GrpcInvokeRequest for golem::workerexecutor::InvokeAndAwaitWorkerRequest {
    fn account_id(&self) -> Result<AccountId, GolemError> {
        Ok(self
            .account_id
            .clone()
            .ok_or(GolemError::invalid_request("account_id not found"))?
            .into())
    }

    fn account_limits(&self) -> Option<GrpcResourceLimits> {
        self.account_limits.clone()
    }

    fn calling_convention(&self) -> CallingConvention {
        match self.calling_convention() {
            golem::worker::CallingConvention::Component => CallingConvention::Component,
            golem::worker::CallingConvention::Stdio => CallingConvention::Stdio,
        }
    }

    fn input(&self) -> Vec<Val> {
        self.input.clone()
    }

    fn worker_id(&self) -> Result<common_model::WorkerId, GolemError> {
        self.worker_id
            .clone()
            .ok_or(GolemError::invalid_request("worker_id not found"))?
            .try_into()
            .map_err(GolemError::invalid_request)
    }

    fn idempotency_key(&self) -> Result<Option<IdempotencyKey>, GolemError> {
        Ok(self.idempotency_key.clone().map(IdempotencyKey::from))
    }

    fn name(&self) -> String {
        self.name.clone()
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

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
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
