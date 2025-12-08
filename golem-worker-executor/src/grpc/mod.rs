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

mod invocation;

use crate::grpc::invocation::{CanStartWorker, GrpcInvokeRequest};
use crate::model::event::InternalWorkerEvent;
use crate::model::public_oplog::{
    find_component_revision_at, get_public_oplog_chunk, search_public_oplog,
};
use crate::model::{LastError, ReadFileResult};
use crate::services::events::Event;
use crate::services::worker_activator::{DefaultWorkerActivator, LazyWorkerActivator};
use crate::services::worker_event::WorkerEventReceiver;
use crate::services::{
    All, HasActiveWorkers, HasAll, HasComponentService, HasEvents, HasOplogService,
    HasPromiseService, HasRunningWorkerEnumerationService, HasShardManagerService, HasShardService,
    HasWorkerEnumerationService, HasWorkerService, UsesAllDeps,
};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use futures::Stream;
use futures::StreamExt;
use gethostname::gethostname;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::worker::{Cursor, UpdateMode};
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_server::WorkerExecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ActivatePluginRequest, ActivatePluginResponse, CancelInvocationRequest,
    CancelInvocationResponse, ConnectWorkerRequest, DeactivatePluginRequest,
    DeactivatePluginResponse, DeleteWorkerRequest, ForkWorkerRequest, ForkWorkerResponse,
    GetFileContentsRequest, GetFileContentsResponse, GetFileSystemNodeRequest,
    GetFileSystemNodeResponse, GetOplogRequest, GetOplogResponse, GetRunningWorkersMetadataRequest,
    GetRunningWorkersMetadataResponse, GetWorkersMetadataRequest, GetWorkersMetadataResponse,
    InvokeAndAwaitWorkerJsonRequest, InvokeAndAwaitWorkerRequest,
    InvokeAndAwaitWorkerResponseTyped, InvokeAndAwaitWorkerSuccess, InvokeJsonWorkerRequest,
    InvokeWorkerResponse, RevertWorkerRequest, RevertWorkerResponse, SearchOplogRequest,
    SearchOplogResponse, UpdateWorkerRequest, UpdateWorkerResponse,
};
use golem_common::grpc::{
    proto_component_id_string, proto_idempotency_key_string, proto_promise_id_string,
    proto_worker_id_string,
};
use golem_common::metrics::api::record_new_grpc_api_active_stream;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentId, AgentMode};
use golem_common::model::component::ComponentRevision;
use golem_common::model::component::{ComponentFilePath, ComponentId, PluginPriority};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{OplogIndex, UpdateDescription};
use golem_common::model::protobuf::to_protobuf_resource_description;
use golem_common::model::{
    IdempotencyKey, OwnedWorkerId, ScanCursor, ShardId, Timestamp, TimestampedWorkerInvocation,
    WorkerEvent, WorkerFilter, WorkerId, WorkerInvocation, WorkerMetadata, WorkerStatus,
};
use golem_common::{model as common_model, recorded_grpc_api_request};
use golem_service_base::error::worker_executor::*;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::GetFileSystemNodeResult;
use golem_wasm::protobuf::Val;
use golem_wasm::ValueAndType;
use std::cmp::min;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};
use tracing::info_span;
use tracing::{debug, info, warn, Instrument};
use wasmtime::Error;

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

    async fn ensure_not_failed(
        &self,
        owned_worker_id: &OwnedWorkerId,
        metadata: &WorkerMetadata,
    ) -> Result<(), WorkerExecutorError> {
        match &metadata.last_known_status.status {
            WorkerStatus::Failed => {
                let error_and_retry_count = Ctx::get_last_error_and_retry_count(
                    self,
                    owned_worker_id,
                    &metadata.last_known_status,
                )
                .await;
                if let Some(last_error) = error_and_retry_count {
                    Err(WorkerExecutorError::PreviousInvocationFailed {
                        error: last_error.error,
                        stderr: last_error.stderr,
                    })
                } else {
                    // TODO: In what cases can we reach here?
                    Err(WorkerExecutorError::runtime(
                        "Previous invocation failed, but failed to get error details",
                    ))
                }
            }
            WorkerStatus::Exited => Err(WorkerExecutorError::PreviousInvocationExited),
            _ => {
                let error_and_retry_count = Ctx::get_last_error_and_retry_count(
                    self,
                    owned_worker_id,
                    &metadata.last_known_status,
                )
                .await;
                debug!("Last error and retry count: {:?}", error_and_retry_count);
                if let Some(last_error) = error_and_retry_count {
                    Err(WorkerExecutorError::PreviousInvocationFailed {
                        error: last_error.error,
                        stderr: last_error.stderr,
                    })
                } else {
                    Ok(())
                }
            }
        }
    }

    fn ensure_worker_belongs_to_this_executor(
        &self,
        worker_id: impl AsRef<WorkerId>,
    ) -> Result<(), WorkerExecutorError> {
        self.shard_service().check_worker(worker_id.as_ref())
    }

    async fn create_worker_internal(
        &self,
        request: golem::workerexecutor::v1::CreateWorkerRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        let component_version: ComponentRevision = ComponentRevision(request.component_version);

        let existing_worker = self.worker_service().get(&owned_worker_id).await;
        if existing_worker.is_some() && !request.ignore_already_existing {
            return Err(WorkerExecutorError::worker_already_exists(
                owned_worker_id.worker_id(),
            ));
        }

        let env = request
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let worker = Worker::get_or_create_suspended(
            self,
            auth_ctx.account_id(),
            &owned_worker_id,
            Some(env),
            Some(
                request
                    .wasi_config_vars
                    .ok_or(WorkerExecutorError::invalid_request(
                        "no wasi_config_vars field",
                    ))?
                    .into(),
            ),
            Some(component_version),
            None,
            &InvocationContextStack::fresh(),
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
                Err(RecvError::Closed) => {
                    Err(WorkerExecutorError::unknown("Events subscription closed"))
                }
                Err(RecvError::Lagged(_)) => Err(WorkerExecutorError::unknown(
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
    ) -> Result<golem::workerexecutor::v1::CompletePromiseSuccess, WorkerExecutorError> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        let promise_id = request
            .promise_id
            .as_ref()
            .ok_or(WorkerExecutorError::invalid_request("promise_id not found"))?;

        let owned_worker_id = extract_owned_worker_id(
            &(&request, promise_id.clone()),
            |(_, r)| &r.worker_id,
            |(r, _)| &r.environment_id,
        )?;

        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let data = request.data;

        let promise_id: common_model::PromiseId = promise_id
            .clone()
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let completed = self
            .promise_service()
            .complete(promise_id, data, auth_ctx.account_id())
            .await?;

        let success = golem::workerexecutor::v1::CompletePromiseSuccess { completed };

        Ok(success)
    }

    async fn delete_worker_internal(
        &self,
        request: DeleteWorkerRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;
        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        if Worker::<Ctx>::get_latest_metadata(&self.services, &owned_worker_id)
            .await
            .is_some()
        {
            let worker = Worker::get_or_create_suspended(
                self,
                auth_ctx.account_id(),
                &owned_worker_id,
                None,
                None,
                None,
                None,
                &InvocationContextStack::fresh(),
            )
            .await?;

            info!("Interrupting worker before deletion");
            if let Some(mut rx) = worker
                .set_interrupting(InterruptKind::Interrupt(Timestamp::now_utc()))
                .await
            {
                info!("Awaiting interruption");
                let _ = rx.recv().await;
                info!("Interrupted");
            }
            info!("Marking worker for deletion");
            worker.start_deleting().await?;

            self.worker_service().remove(&owned_worker_id).await;
            self.active_workers()
                .remove(&owned_worker_id.worker_id)
                .await;

            // ensure we are holding the worker while we are doing cleanup.
            drop(worker);
        }

        Ok(())
    }

    async fn fork_worker_internal(
        &self,
        request: ForkWorkerRequest,
    ) -> Result<ForkWorkerResponse, WorkerExecutorError> {
        let account_id_proto = request
            .component_owner_account_id
            .ok_or(WorkerExecutorError::invalid_request("account_id not found"))?;

        let account_id: AccountId = account_id_proto.try_into().map_err(|e| {
            WorkerExecutorError::invalid_request(format!("Invalid account id: {e}"))
        })?;

        let environment_id_proto = request
            .environment_id
            .ok_or(WorkerExecutorError::invalid_request("project_id not found"))?;
        let environment_id: EnvironmentId = environment_id_proto
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let target_worker_id_proto = request
            .target_worker_id
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("worker_id not found"))?;

        let target_worker_id: WorkerId = target_worker_id_proto
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let owned_target_worker_id = OwnedWorkerId::new(&environment_id, &target_worker_id);

        let source_worker_id_proto = request
            .source_worker_id
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("worker_id not found"))?;

        let source_worker_id: WorkerId = source_worker_id_proto
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let owned_source_worker_id = OwnedWorkerId::new(&environment_id, &source_worker_id);

        self.services
            .worker_fork_service()
            .fork(
                &account_id,
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

    async fn revert_worker_internal(
        &self,
        request: RevertWorkerRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        let target = request
            .target
            .ok_or(WorkerExecutorError::invalid_request("target not found"))?;

        let target = target
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;

        let metadata = self.worker_service().get(&owned_worker_id).await;

        match metadata {
            Some(_) => {
                let worker = Worker::get_or_create_suspended(
                    self,
                    auth_ctx.account_id(),
                    &owned_worker_id,
                    None,
                    None,
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                )
                .await?;
                worker.revert(target).await?;
                Ok(())
            }
            None => Err(WorkerExecutorError::worker_not_found(
                owned_worker_id.worker_id(),
            )),
        }
    }

    async fn cancel_invocation_internal(
        &self,
        request: CancelInvocationRequest,
    ) -> Result<bool, WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        let idempotency_key = request
            .idempotency_key
            .ok_or(WorkerExecutorError::invalid_request(
                "idempotency_key not found",
            ))?
            .into();

        let metadata = Worker::<Ctx>::get_latest_metadata(&self.services, &owned_worker_id)
            .await
            .ok_or(WorkerExecutorError::worker_not_found(
                owned_worker_id.worker_id(),
            ))?;

        if metadata
            .last_known_status
            .pending_invocations
            .iter()
            .any(|invocation| invocation.invocation.idempotency_key() == Some(&idempotency_key))
        {
            let worker = Worker::get_or_create_suspended(
                self,
                auth_ctx.account_id(),
                &owned_worker_id,
                None,
                None,
                None,
                None,
                &InvocationContextStack::fresh(),
            )
            .await?;
            worker.cancel_invocation(idempotency_key).await?;
            Ok(true)
        } else if metadata
            .last_known_status
            .invocation_results
            .contains_key(&idempotency_key)
        {
            Ok(false)
        } else {
            Err(WorkerExecutorError::invalid_request("Invocation not found"))
        }
    }

    async fn interrupt_worker_internal(
        &self,
        request: golem::workerexecutor::v1::InterruptWorkerRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        let metadata = Worker::<Ctx>::get_latest_metadata(&self.services, &owned_worker_id).await;

        if let Some(metadata) = metadata {
            match &metadata.last_known_status.status {
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
                    let worker = Worker::get_or_create_suspended(
                        self,
                        auth_ctx.account_id(),
                        &owned_worker_id,
                        None,
                        None,
                        None,
                        None,
                        &InvocationContextStack::fresh(),
                    )
                    .await?;
                    if let Some(mut await_interruption) = worker
                        .set_interrupting(InterruptKind::Interrupt(Timestamp::now_utc()))
                        .await
                    {
                        await_interruption.recv().await.unwrap();
                    };
                    // Explicitly drop from the active worker cache - this will drop websocket connections etc.
                    self.active_workers()
                        .remove(&owned_worker_id.worker_id)
                        .await;
                }
                WorkerStatus::Retrying => {
                    debug!("Marking worker scheduled to be retried as interrupted");
                    let worker = Worker::get_or_create_suspended(
                        self,
                        auth_ctx.account_id(),
                        &owned_worker_id,
                        None,
                        None,
                        None,
                        None,
                        &InvocationContextStack::fresh(),
                    )
                    .await?;
                    if let Some(mut await_interruption) = worker
                        .set_interrupting(InterruptKind::Interrupt(Timestamp::now_utc()))
                        .await
                    {
                        await_interruption.recv().await.unwrap();
                    };
                    // Explicitly drop from the active worker cache - this will drop websocket connections etc.
                    self.active_workers()
                        .remove(&owned_worker_id.worker_id)
                        .await;
                }
                WorkerStatus::Running => {
                    let worker = Worker::get_or_create_suspended(
                        self,
                        auth_ctx.account_id(),
                        &owned_worker_id,
                        None,
                        None,
                        None,
                        None,
                        &InvocationContextStack::fresh(),
                    )
                    .await?;
                    if let Some(mut await_interruption) = worker
                        .set_interrupting(if request.recover_immediately {
                            InterruptKind::Restart
                        } else {
                            InterruptKind::Interrupt(Timestamp::now_utc())
                        })
                        .await
                    {
                        await_interruption.recv().await.unwrap();
                    };

                    // Explicitly drop from the active worker cache - this will drop websocket connections etc.
                    self.active_workers()
                        .remove(&owned_worker_id.worker_id)
                        .await;
                }
            }
        }
        Ok(())
    }

    async fn resume_worker_internal(
        &self,
        request: golem::workerexecutor::v1::ResumeWorkerRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        let force_resume = request.force.unwrap_or(false);

        let metadata = Worker::<Ctx>::get_latest_metadata(&self.services, &owned_worker_id)
            .await
            .ok_or(WorkerExecutorError::worker_not_found(
                owned_worker_id.worker_id(),
            ))?;

        self.ensure_not_failed(&owned_worker_id, &metadata).await?;

        match &metadata.last_known_status.status {
            WorkerStatus::Suspended | WorkerStatus::Interrupted | WorkerStatus::Idle => {
                info!(
                    "Activating {:?} worker {owned_worker_id} due to explicit resume request",
                    metadata.last_known_status.status
                );
                let _ = Worker::get_or_create_running(
                    &self.services,
                    auth_ctx.account_id(),
                    &owned_worker_id,
                    None,
                    None,
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                )
                .await?;
                Ok(())
            }
            _ if force_resume => {
                info!(
                    "Force activating {:?} worker {owned_worker_id} due to explicit resume request",
                    metadata.last_known_status.status
                );
                let _ = Worker::get_or_create_running(
                    &self.services,
                    auth_ctx.account_id(),
                    &owned_worker_id,
                    None,
                    None,
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                )
                .await?;
                Ok(())
            }
            _ => Err(WorkerExecutorError::invalid_request(format!(
                "Worker {worker_id} is not suspended, interrupted or idle",
                worker_id = owned_worker_id.worker_id
            ))),
        }
    }

    async fn invoke_and_await_worker_internal_proto<Req: GrpcInvokeRequest>(
        &self,
        request: &Req,
    ) -> Result<Option<Val>, WorkerExecutorError> {
        let result = self.invoke_and_await_worker_internal_typed(request).await?;
        let value = result
            .map(golem_wasm::Value::try_from)
            .transpose()
            .map_err(|e| WorkerExecutorError::unknown(e.to_string()))?
            .map(|value| value.into());
        Ok(value)
    }

    async fn invoke_and_await_worker_internal_typed<Req: GrpcInvokeRequest>(
        &self,
        request: &Req,
    ) -> Result<Option<ValueAndType>, WorkerExecutorError> {
        let full_function_name = request.name();

        let worker = self.get_or_create(request).await?;

        let idempotency_key = request
            .idempotency_key()?
            .unwrap_or(IdempotencyKey::fresh());

        let params_val = request.input(&worker).await?;

        let function_input = params_val
            .into_iter()
            .map(|val| val.clone().try_into())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|msg| WorkerExecutorError::ValueMismatch { details: msg })?;

        let value = worker
            .invoke_and_await(
                idempotency_key,
                full_function_name,
                function_input,
                request.invocation_context(),
            )
            .await?;

        Ok(value)
    }

    async fn get_or_create<Req: CanStartWorker>(
        &self,
        request: &Req,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        let worker = self.get_or_create_pending(request).await?;
        Worker::start_if_needed(worker.clone()).await?;
        Ok(worker)
    }

    async fn get_or_create_pending<Req: CanStartWorker>(
        &self,
        request: &Req,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        let worker_id = request.worker_id()?;
        let project_id = request.environment_id()?;

        let owned_worker_id = OwnedWorkerId::new(&project_id, &worker_id);
        self.ensure_worker_belongs_to_this_executor(&worker_id)?;

        let auth_ctx = request.auth_ctx()?;

        let metadata = Worker::<Ctx>::get_latest_metadata(self, &owned_worker_id).await;

        if let Some(metadata) = &metadata {
            self.ensure_not_failed(&owned_worker_id, metadata).await?;
        }

        let invocation_context = request
            .maybe_invocation_context()
            .unwrap_or_else(InvocationContextStack::fresh);

        Worker::get_or_create_suspended(
            self,
            auth_ctx.account_id(),
            &owned_worker_id,
            request.env(),
            request.wasi_config_vars()?,
            None,
            request.parent(),
            &invocation_context,
        )
        .await
    }

    async fn invoke_worker_internal<Req: GrpcInvokeRequest>(
        &self,
        request: &Req,
    ) -> Result<(), WorkerExecutorError> {
        let full_function_name = request.name();

        let worker = self.get_or_create(request).await?;

        let idempotency_key = request
            .idempotency_key()?
            .unwrap_or(IdempotencyKey::fresh());

        let function_input = request
            .input(&worker)
            .await?
            .iter()
            .map(|val| val.clone().try_into())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|msg| WorkerExecutorError::ValueMismatch { details: msg })?;

        worker
            .invoke(
                idempotency_key,
                full_function_name,
                function_input,
                request.invocation_context(),
            )
            .await?;

        Ok(())
    }

    async fn revoke_shards_internal(
        &self,
        request: golem::workerexecutor::v1::RevokeShardsRequest,
    ) -> Result<(), WorkerExecutorError> {
        let proto_shard_ids = request.shard_ids;

        let shard_ids = proto_shard_ids.into_iter().map(ShardId::from).collect();

        self.shard_service().revoke_shards(&shard_ids)?;

        for (worker_id, worker_details) in self.active_workers().snapshot().await {
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
    ) -> Result<(), WorkerExecutorError> {
        let proto_shard_ids = request.shard_ids;

        let shard_ids = proto_shard_ids.into_iter().map(ShardId::from).collect();

        self.shard_service().assign_shards(&shard_ids)?;
        Ctx::on_shard_assignment_changed(self).await?;

        Ok(())
    }

    async fn get_worker_metadata_internal(
        &self,
        request: golem::workerexecutor::v1::GetWorkerMetadataRequest,
    ) -> Result<golem::worker::WorkerMetadata, WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;
        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let metadata = Worker::<Ctx>::get_latest_metadata(self, &owned_worker_id)
            .await
            .ok_or(WorkerExecutorError::worker_not_found(
                owned_worker_id.worker_id(),
            ))?;

        let last_error_and_retry_count = Ctx::get_last_error_and_retry_count(
            self,
            &owned_worker_id,
            &metadata.last_known_status,
        )
        .await;

        Self::create_proto_metadata(metadata, last_error_and_retry_count)
    }

    async fn get_running_workers_metadata_internal(
        &self,
        request: GetRunningWorkersMetadataRequest,
    ) -> Result<Vec<golem::worker::WorkerMetadata>, WorkerExecutorError> {
        let component_id: ComponentId = request
            .component_id
            .and_then(|t| t.try_into().ok())
            .ok_or(WorkerExecutorError::invalid_request("Invalid component id"))?;

        let filter: Option<WorkerFilter> = match request.filter {
            Some(f) => Some(f.try_into().map_err(WorkerExecutorError::invalid_request)?),
            _ => None,
        };

        let workers = self
            .running_worker_enumeration_service()
            .get(&component_id, filter)
            .await?;

        let result: Vec<golem::worker::WorkerMetadata> = workers
            .into_iter()
            .map(|worker_metadata| Self::create_proto_metadata(worker_metadata, None))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(result)
    }

    async fn get_workers_metadata_internal(
        &self,
        request: GetWorkersMetadataRequest,
    ) -> Result<(Option<Cursor>, Vec<golem::worker::WorkerMetadata>), WorkerExecutorError> {
        let component_id: ComponentId = request
            .component_id
            .and_then(|t| t.try_into().ok())
            .ok_or(WorkerExecutorError::invalid_request("Invalid component id"))?;

        let environment_id: EnvironmentId = request
            .environment_id
            .and_then(|t| t.try_into().ok())
            .ok_or(WorkerExecutorError::invalid_request("Invalid project id"))?;

        let filter: Option<WorkerFilter> = match request.filter {
            Some(f) => Some(f.try_into().map_err(WorkerExecutorError::invalid_request)?),
            _ => None,
        };

        let (new_cursor, workers) = self
            .worker_enumeration_service()
            .get(
                &environment_id,
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

        for worker_metadata in workers {
            let last_error_and_retry_count = Ctx::get_last_error_and_retry_count(
                self,
                &worker_metadata.owned_worker_id(),
                &worker_metadata.last_known_status,
            )
            .await;
            let metadata =
                Self::create_proto_metadata(worker_metadata, last_error_and_retry_count)?;
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

    async fn update_worker_internal(
        &self,
        request: UpdateWorkerRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        let metadata = Worker::<Ctx>::get_latest_metadata(self, &owned_worker_id)
            .await
            .ok_or(WorkerExecutorError::worker_not_found(
                owned_worker_id.worker_id(),
            ))?;

        if metadata.last_known_status.component_revision.0 == request.target_version {
            return Err(WorkerExecutorError::invalid_request(
                "Worker is already at the target version",
            ));
        }

        let component_metadata = self
            .component_service()
            .get_metadata(
                &owned_worker_id.worker_id.component_id,
                Some(metadata.last_known_status.component_revision),
            )
            .await?;

        if let Ok(agent_id) = AgentId::parse(
            &owned_worker_id.worker_id.worker_name,
            &component_metadata.metadata,
        ) {
            if let Ok(Some(agent_type)) = component_metadata
                .metadata
                .find_agent_type_by_name(&agent_id.agent_type)
            {
                if agent_type.mode == AgentMode::Ephemeral {
                    return Err(WorkerExecutorError::invalid_request(
                        "Ephemeral workers cannot be updated",
                    ));
                }
            }
        }

        match request.mode() {
            UpdateMode::Automatic => {
                let update_description = UpdateDescription::Automatic {
                    target_revision: ComponentRevision(request.target_version),
                };

                if metadata
                    .last_known_status
                    .pending_updates
                    .iter()
                    .any(|update| update.description == update_description)
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "The same update is already in progress",
                    ));
                }

                match &metadata.last_known_status.status {
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
                            auth_ctx.account_id(),
                            &owned_worker_id,
                            None,
                            None,
                            Some(metadata.last_known_status.component_revision),
                            None,
                            &InvocationContextStack::fresh(),
                        )
                        .await?;

                        debug!("Enqueuing update");
                        worker.enqueue_update(update_description.clone()).await;

                        debug!("Resuming initialization to perform the update",);
                        Worker::start_if_needed(worker.clone()).await?;
                    }
                    WorkerStatus::Running | WorkerStatus::Idle => {
                        // If the worker is already running we need to write to its oplog the
                        // update attempt, and then interrupt it and have it immediately restarting
                        // to begin the update.
                        let worker = Worker::get_or_create_suspended(
                            self,
                            auth_ctx.account_id(),
                            &owned_worker_id,
                            None,
                            None,
                            None,
                            None,
                            &InvocationContextStack::fresh(),
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
                    matches!(invocation, TimestampedWorkerInvocation { invocation: WorkerInvocation::ManualUpdate { target_revision, .. }, ..} if target_revision.0 == request.target_version)
                ) {
                    return Err(WorkerExecutorError::invalid_request(
                        "The same update is already in progress",
                    ));
                }

                // For manual update we need to invoke the worker to save the custom snapshot.
                // This is in a race condition with other worker invocations, so the whole update
                // process need to be initiated through the worker's invocation queue.

                let worker = Worker::get_or_create_suspended(
                    self,
                    auth_ctx.account_id(),
                    &owned_worker_id,
                    None,
                    None,
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                )
                .await?;
                worker
                    .enqueue_manual_update(ComponentRevision(request.target_version))
                    .await?;
            }
        }

        Ok(())
    }

    async fn connect_worker_internal(
        &self,
        request: ConnectWorkerRequest,
    ) -> ResponseResult<<Self as WorkerExecutor>::ConnectWorkerStream> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        let metadata = Worker::<Ctx>::get_latest_metadata(self, &owned_worker_id)
            .await
            .ok_or(WorkerExecutorError::worker_not_found(
                owned_worker_id.worker_id(),
            ))?;

        self.ensure_not_failed(&owned_worker_id, &metadata).await?;

        if metadata.last_known_status.status != WorkerStatus::Interrupted {
            let event_service = Worker::get_or_create_suspended(
                self,
                auth_ctx.account_id(),
                &owned_worker_id,
                None,
                None,
                None,
                None,
                &InvocationContextStack::fresh(),
            )
            .await?
            .event_service();

            let receiver = event_service.receiver();

            info!("Client connected");
            record_new_grpc_api_active_stream();

            Ok(Response::new(WorkerEventStream::new(receiver)))
        } else {
            // We don't want 'connect' to resume interrupted workers
            Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Interrupt(Timestamp::now_utc()),
            }
            .into())
        }
    }

    async fn get_oplog_internal(
        &self,
        request: GetOplogRequest,
    ) -> Result<GetOplogResponse, WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;
        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let chunk = match request.cursor {
            Some(cursor) => get_public_oplog_chunk(
                self.component_service(),
                self.oplog_service(),
                &owned_worker_id,
                ComponentRevision(cursor.current_component_version),
                OplogIndex::from_u64(cursor.next_oplog_index),
                min(
                    request.count as usize,
                    self.services.config().limits.max_oplog_query_pages_size,
                ),
            )
            .await
            .map_err(WorkerExecutorError::unknown)?,
            None => {
                let start = OplogIndex::from_u64(request.from_oplog_index);
                let initial_component_version =
                    find_component_revision_at(self.oplog_service(), &owned_worker_id, start)
                        .await?;

                get_public_oplog_chunk(
                    self.component_service(),
                    self.oplog_service(),
                    &owned_worker_id,
                    initial_component_version,
                    start,
                    min(
                        request.count as usize,
                        self.services.config().limits.max_oplog_query_pages_size,
                    ),
                )
                .await
                .map_err(WorkerExecutorError::unknown)?
            }
        };

        let next = if chunk.entries.is_empty() {
            None
        } else {
            Some(golem::worker::OplogCursor {
                next_oplog_index: chunk.next_oplog_index.into(),
                current_component_version: chunk.current_component_revision.0,
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
                            .map_err(WorkerExecutorError::unknown)?,
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
    ) -> Result<SearchOplogResponse, WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;
        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let chunk = match request.cursor {
            Some(cursor) => search_public_oplog(
                self.component_service(),
                self.oplog_service(),
                &owned_worker_id,
                ComponentRevision(cursor.current_component_version),
                OplogIndex::from_u64(cursor.next_oplog_index),
                min(
                    request.count as usize,
                    self.services.config().limits.max_oplog_query_pages_size,
                ),
                &request.query,
            )
            .await
            .map_err(WorkerExecutorError::unknown)?,
            None => {
                let start = OplogIndex::INITIAL;
                let initial_component_version =
                    find_component_revision_at(self.oplog_service(), &owned_worker_id, start)
                        .await?;
                search_public_oplog(
                    self.component_service(),
                    self.oplog_service(),
                    &owned_worker_id,
                    initial_component_version,
                    start,
                    min(
                        request.count as usize,
                        self.services.config().limits.max_oplog_query_pages_size,
                    ),
                    &request.query,
                )
                .await
                .map_err(WorkerExecutorError::unknown)?
            }
        };

        let next = if chunk.entries.is_empty() {
            None
        } else {
            Some(golem::worker::OplogCursor {
                next_oplog_index: chunk.next_oplog_index.into(),
                current_component_version: chunk.current_component_revision.0,
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
                            .map_err(WorkerExecutorError::unknown)?,
                        next,
                        last_index: chunk.last_index.into(),
                    },
                ),
            ),
        })
    }

    async fn get_file_system_node_internal(
        &self,
        request: GetFileSystemNodeRequest,
    ) -> Result<GetFileSystemNodeResponse, WorkerExecutorError> {
        let path = ComponentFilePath::from_abs_str(&request.path)
            .map_err(|e| WorkerExecutorError::invalid_request(format!("Invalid path: {e}")))?;

        let worker = self.get_or_create(&request).await?;

        let result = worker.get_file_system_node(path).await?;

        let response = match result {
            GetFileSystemNodeResult::Ok(entries) => GetFileSystemNodeResponse {
                result: Some(
                    golem::workerexecutor::v1::get_file_system_node_response::Result::DirSuccess(
                        golem::workerexecutor::v1::ListDirectorySuccessResponse {
                            nodes: entries.into_iter().map(|entry| entry.into()).collect(),
                        },
                    ),
                ),
            },
            GetFileSystemNodeResult::NotFound => GetFileSystemNodeResponse {
                result: Some(
                    golem::workerexecutor::v1::get_file_system_node_response::Result::NotFound(
                        golem::common::Empty {},
                    ),
                ),
            },
            GetFileSystemNodeResult::File(file_node) => GetFileSystemNodeResponse {
                result: Some(
                    golem::workerexecutor::v1::get_file_system_node_response::Result::FileSuccess(
                        golem::workerexecutor::v1::ListFileDataSuccessResponse {
                            file: Some(file_node.into()),
                        },
                    ),
                ),
            },
        };

        Ok(response)
    }

    async fn get_file_contents_internal(
        &self,
        request: GetFileContentsRequest,
    ) -> Result<<Self as WorkerExecutor>::GetFileContentsStream, WorkerExecutorError> {
        let path = ComponentFilePath::from_abs_str(&request.file_path)
            .map_err(|e| WorkerExecutorError::invalid_request(format!("Invalid path: {e}")))?;

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
    ) -> Result<(), WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        let plugin_priority: PluginPriority = PluginPriority(request.plugin_priority);

        let metadata = Worker::get_latest_metadata(&self.services, &owned_worker_id).await;

        match metadata {
            Some(metadata) => {
                // Worker exists
                if metadata
                    .last_known_status
                    .active_plugins
                    .contains(&plugin_priority)
                {
                    warn!("Plugin is already activated");
                    Ok(())
                } else {
                    let component_metadata = self
                        .component_service()
                        .get_metadata(
                            &owned_worker_id.worker_id.component_id,
                            Some(metadata.last_known_status.component_revision),
                        )
                        .await?;

                    if component_metadata
                        .installed_plugins
                        .iter()
                        .any(|installation| installation.priority == plugin_priority)
                    {
                        let worker = Worker::get_or_create_suspended(
                            self,
                            auth_ctx.account_id(),
                            &owned_worker_id,
                            None,
                            None,
                            None,
                            None,
                            &InvocationContextStack::fresh(),
                        )
                        .await?;
                        worker.activate_plugin(plugin_priority).await?;
                        Ok(())
                    } else {
                        Err(WorkerExecutorError::invalid_request(
                            "Plugin installation does not belong to this worker's component",
                        ))
                    }
                }
            }
            None => Err(WorkerExecutorError::worker_not_found(
                owned_worker_id.worker_id(),
            )),
        }
    }

    async fn deactivate_plugin_internal(
        &self,
        request: DeactivatePluginRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_worker_id =
            extract_owned_worker_id(&request, |r| &r.worker_id, |r| &r.environment_id)?;
        self.ensure_worker_belongs_to_this_executor(&owned_worker_id)?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
            })?;

        let plugin_priority = PluginPriority(request.plugin_priority);

        let metadata = Worker::<Ctx>::get_latest_metadata(&self.services, &owned_worker_id)
            .await
            .ok_or(WorkerExecutorError::worker_not_found(
                owned_worker_id.worker_id(),
            ))?;

        // Worker exists
        if !metadata
            .last_known_status
            .active_plugins
            .contains(&plugin_priority)
        {
            warn!("Plugin is already deactivated");
            Ok(())
        } else {
            let component_metadata = self
                .component_service()
                .get_metadata(
                    &owned_worker_id.worker_id.component_id,
                    Some(metadata.last_known_status.component_revision),
                )
                .await?;

            if component_metadata
                .installed_plugins
                .iter()
                .any(|installation| installation.priority == plugin_priority)
            {
                let worker = Worker::get_or_create_suspended(
                    self,
                    auth_ctx.account_id(),
                    &owned_worker_id,
                    None,
                    None,
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                )
                .await?;
                worker.deactivate_plugin(plugin_priority).await?;
                Ok(())
            } else {
                Err(WorkerExecutorError::invalid_request(
                    "Plugin installation does not belong to this worker's component",
                ))
            }
        }
    }

    fn create_proto_metadata(
        metadata: WorkerMetadata,
        last_error_and_retry_count: Option<LastError>,
    ) -> Result<golem::worker::WorkerMetadata, WorkerExecutorError> {
        let mut updates = Vec::new();

        let latest_status = metadata.last_known_status;
        for pending_invocation in &latest_status.pending_invocations {
            if let TimestampedWorkerInvocation {
                timestamp,
                invocation: WorkerInvocation::ManualUpdate { target_revision },
            } = pending_invocation
            {
                updates.push(golem::worker::UpdateRecord {
                    timestamp: Some((*timestamp).into()),
                    target_version: target_revision.0,
                    update: Some(golem::worker::update_record::Update::Pending(
                        golem::worker::PendingUpdate {},
                    )),
                });
            }
        }
        for pending_update in &latest_status.pending_updates {
            updates.push(golem::worker::UpdateRecord {
                timestamp: Some(pending_update.timestamp.into()),
                target_version: pending_update.description.target_revision().0,
                update: Some(golem::worker::update_record::Update::Pending(
                    golem::worker::PendingUpdate {},
                )),
            });
        }
        for successful_update in &latest_status.successful_updates {
            updates.push(golem::worker::UpdateRecord {
                timestamp: Some(successful_update.timestamp.into()),
                target_version: successful_update.target_revision.0,
                update: Some(golem::worker::update_record::Update::Successful(
                    golem::worker::SuccessfulUpdate {},
                )),
            });
        }
        for failed_update in &latest_status.failed_updates {
            updates.push(golem::worker::UpdateRecord {
                timestamp: Some(failed_update.timestamp.into()),
                target_version: failed_update.target_revision.0,
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

        let mut owned_resources = Vec::new();
        for (resource_id, resource) in latest_status.owned_resources {
            owned_resources.push(to_protobuf_resource_description(resource_id, resource));
        }

        let active_plugins = latest_status.active_plugins;

        Ok(golem::worker::WorkerMetadata {
            worker_id: Some(metadata.worker_id.into()),
            environment_id: Some(metadata.environment_id.into()),
            env: HashMap::from_iter(metadata.env.iter().cloned()),
            created_by: Some(metadata.created_by.into()),
            wasi_config_vars: Some(metadata.wasi_config_vars.into()),
            component_version: latest_status.component_revision.0,
            status: Into::<golem::worker::WorkerStatus>::into(latest_status.status.clone()).into(),
            retry_count: last_error_and_retry_count
                .as_ref()
                .and_then(|last_error| {
                    latest_status
                        .current_retry_count
                        .get(&last_error.retry_from)
                        .copied()
                })
                .unwrap_or_default(),
            pending_invocation_count: latest_status.pending_invocations.len() as u64,
            updates,
            created_at: Some(metadata.created_at.into()),
            last_error: last_error_and_retry_count
                .map(|last_error| last_error.error.to_string(&last_error.stderr)),
            component_size: latest_status.component_size,
            total_linear_memory_size: latest_status.total_linear_memory_size,
            owned_resources,
            active_plugins: active_plugins.into_iter().map(|id| id.0).collect(),
            skipped_regions: latest_status
                .skipped_regions
                .into_regions()
                .map(|region| region.into())
                .collect(),
            deleted_regions: latest_status
                .deleted_regions
                .into_regions()
                .map(|region| region.into())
                .collect(),
        })
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
            component_version = request.component_version
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::CreateWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::create_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            worker_id = proto_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InvokeAndAwaitWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::invoke_and_await_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn invoke_and_await_worker_typed(
        &self,
        request: Request<InvokeAndAwaitWorkerRequest>,
    ) -> Result<Response<InvokeAndAwaitWorkerResponseTyped>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "invoke_and_await_worker_typed",
            worker_id = proto_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
        );

        match self.invoke_and_await_worker_internal_typed(&request).instrument(record.span.clone()).await {
            Ok(value_and_type) => {
                let result = golem::workerexecutor::v1::InvokeAndAwaitWorkerSuccessTyped {
                    output: value_and_type.map(|vnt| vnt.into())
                };

                record.succeed(Ok(Response::new(
                    golem::workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result: Some(
                            golem::workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Success(result),
                        ),
                    },
                )))
            }
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result: Some(
                            golem::workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            worker_id = proto_worker_id_string(&request.worker_id),
            function = request.name,
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InvokeWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::invoke_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn invoke_and_await_worker_json(
        &self,
        request: Request<InvokeAndAwaitWorkerJsonRequest>,
    ) -> Result<Response<InvokeAndAwaitWorkerResponseTyped>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "invoke_and_await_worker_json",
            worker_id = proto_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
        );

        match self.invoke_and_await_worker_internal_typed(&request).instrument(record.span.clone()).await {
            Ok(value_and_type) => {
                let result = golem::workerexecutor::v1::InvokeAndAwaitWorkerSuccessTyped {
                    output: value_and_type.map(|vnt| vnt.into())
                };

                record.succeed(Ok(Response::new(
                    InvokeAndAwaitWorkerResponseTyped {
                        result: Some(
                            golem::workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Success(result),
                        ),
                    },
                )))
            }
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InvokeAndAwaitWorkerResponseTyped {
                        result: Some(
                            golem::workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn invoke_worker_json(
        &self,
        request: Request<InvokeJsonWorkerRequest>,
    ) -> Result<Response<InvokeWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "invoke_worker_json",
            worker_id = proto_worker_id_string(&request.worker_id),
            function = request.name,
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InvokeWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::invoke_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::DeleteWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::delete_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::CompletePromiseResponse {
                        result: Some(
                            golem::workerexecutor::v1::complete_promise_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InterruptWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::interrupt_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::RevokeShardsResponse {
                        result: Some(
                            golem::workerexecutor::v1::revoke_shards_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::AssignShardsResponse {
                        result: Some(
                            golem::workerexecutor::v1::assign_shards_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            Err(err @ WorkerExecutorError::WorkerNotFound { .. }) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::GetWorkerMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_worker_metadata_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                },
            ))),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::GetWorkerMetadataResponse {
                        result: Some(
                            golem::workerexecutor::v1::get_worker_metadata_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::ResumeWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::resume_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(
                    GetRunningWorkersMetadataResponse {
                        result: Some(
                            golem::workerexecutor::v1::get_running_workers_metadata_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(GetWorkersMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_workers_metadata_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(UpdateWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::update_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(GetOplogResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_oplog_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(SearchOplogResponse {
                    result: Some(
                        golem::workerexecutor::v1::search_oplog_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
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
            Err(mut err) => record.fail(
                Ok(Response::new(ForkWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::fork_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn revert_worker(
        &self,
        request: Request<RevertWorkerRequest>,
    ) -> Result<Response<RevertWorkerResponse>, Status> {
        let request = request.into_inner();

        let record = recorded_grpc_api_request!(
            "revert_worker",
            worker_id = proto_worker_id_string(&request.worker_id),
        );

        let result = self
            .revert_worker_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(_) => record.succeed(Ok(Response::new(RevertWorkerResponse {
                result: Some(
                    golem::workerexecutor::v1::revert_worker_response::Result::Success(
                        golem::common::Empty {},
                    ),
                ),
            }))),
            Err(mut err) => record.fail(
                Ok(Response::new(RevertWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::revert_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn cancel_invocation(
        &self,
        request: Request<CancelInvocationRequest>,
    ) -> Result<Response<CancelInvocationResponse>, Status> {
        let request = request.into_inner();

        let record = recorded_grpc_api_request!(
            "cancel_invocation",
            worker_id = proto_worker_id_string(&request.worker_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
        );

        let result = self
            .cancel_invocation_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(canceled) => record.succeed(Ok(Response::new(CancelInvocationResponse {
                result: Some(
                    golem::workerexecutor::v1::cancel_invocation_response::Result::Success(
                        canceled,
                    ),
                ),
            }))),
            Err(mut err) => record.fail(
                Ok(Response::new(CancelInvocationResponse {
                    result: Some(
                        golem::workerexecutor::v1::cancel_invocation_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn get_file_system_node(
        &self,
        request: Request<GetFileSystemNodeRequest>,
    ) -> ResponseResult<GetFileSystemNodeResponse> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_file_system_node",
            worker_id = proto_worker_id_string(&request.worker_id),
            path = request.path,
        );

        let result = self
            .get_file_system_node_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(response) => record.succeed(Ok(Response::new(response))),
            Err(mut err) => record.fail(
                Ok(Response::new(GetFileSystemNodeResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_file_system_node_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
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
            worker_id = proto_worker_id_string(&request.worker_id),
            path = request.file_path,
        );

        let result = self
            .get_file_contents_internal(request)
            .instrument(record.span.clone())
            .await;

        let stream: Self::GetFileContentsStream = match result {
            Ok(stream) => record.succeed(stream),
            Err(mut err) => {
                let res = GetFileContentsResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_file_contents_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                };

                let err_stream: Self::GetFileContentsStream =
                    Box::pin(tokio_stream::iter(vec![Ok(res)]));

                record.fail(err_stream, &mut err)
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
            plugin_priority = request.plugin_priority
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
            Err(mut err) => record.fail(
                Ok(Response::new(ActivatePluginResponse {
                    result: Some(
                        golem::workerexecutor::v1::activate_plugin_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
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
            plugin_priority = request.plugin_priority
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
            Err(mut err) => record.fail(
                Ok(Response::new(DeactivatePluginResponse {
                    result: Some(
                        golem::workerexecutor::v1::deactivate_plugin_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }
}

pub struct WorkerEventStream {
    inner:
        Pin<Box<dyn Stream<Item = Result<InternalWorkerEvent, BroadcastStreamRecvError>> + Send>>,
}

impl WorkerEventStream {
    pub fn new(receiver: WorkerEventReceiver) -> Self {
        WorkerEventStream {
            inner: Box::pin(receiver.to_stream()),
        }
    }
}

impl Stream for WorkerEventStream {
    type Item = Result<golem::worker::LogEvent, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let WorkerEventStream { inner } = self.get_mut();
        match inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => {
                Poll::Ready(Some(Ok(WorkerEvent::from(event).try_into().unwrap())))
            }
            Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(n)))) => {
                Poll::Ready(Some(Ok(WorkerEvent::ClientLagged {
                    number_of_missed_messages: n,
                }
                .try_into()
                .unwrap())))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn extract_owned_worker_id<T>(
    request: &T,
    get_worker_id: impl FnOnce(&T) -> &Option<golem::worker::WorkerId>,
    get_environment_id: impl FnOnce(&T) -> &Option<golem::common::EnvironmentId>,
) -> Result<OwnedWorkerId, WorkerExecutorError> {
    let worker_id = get_worker_id(request)
        .as_ref()
        .ok_or(WorkerExecutorError::invalid_request("worker_id not found"))?;
    let worker_id = worker_id
        .clone()
        .try_into()
        .map_err(WorkerExecutorError::invalid_request)?;

    let environment_id =
        get_environment_id(request)
            .as_ref()
            .ok_or(WorkerExecutorError::invalid_request(
                "environment_id not found",
            ))?;
    let environment_id: EnvironmentId = (*environment_id)
        .try_into()
        .map_err(WorkerExecutorError::invalid_request)?;

    Ok(OwnedWorkerId::new(&environment_id, &worker_id))
}
