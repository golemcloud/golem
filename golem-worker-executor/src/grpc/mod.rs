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

mod invocation;
mod invocation_session;
pub(crate) mod stream_slots;

pub(crate) use invocation::{CanStartWorker, from_proto_invocation_context};
pub(crate) use invocation_session::{build_durable_streaming_request, decode_invocation_input};

use crate::durable_host::agent_monomorphization_context;
use crate::model::event::InternalWorkerEvent;
use crate::model::public_oplog::{
    PublicOplogPageLimits, find_component_revision_at, get_public_oplog_entry,
    get_public_oplog_page, search_public_oplog_page,
};
use crate::model::{LastError, LookupResult};
use crate::services::events::Event;
use crate::services::rpc::DurableStreamRemoteError;
use crate::services::shard_manager::{RecoveryOutcome, ShardAssignmentChangedHook};
use crate::services::worker_activator::{
    DefaultWorkerActivator, LazyWorkerActivator, WorkerActivator,
};
use crate::services::worker_event::WorkerEventReceiver;
use crate::services::{
    All, HasActiveAgents, HasAll, HasComponentService, HasConfig, HasEvents, HasOplog,
    HasOplogService, HasPromiseService, HasRunningWorkerEnumerationService, HasShardManagerService,
    HasShardService, HasWorkerEnumerationService, HasWorkerService, UsesAllDeps,
};
use crate::worker::{
    ExportStreamControlResult as DomainExportResult, RetirementReason, Worker, WorkerUpdateMode,
    retired_by_assignment,
};
pub use crate::worker::{
    PERMISSION_CARD_INSTALL_RECIPIENT_MISMATCH, PERMISSION_CARD_TRANSFER_PAYLOAD_CONFLICT,
};
use crate::workerctx::WorkerCtx;
use futures::Stream;
use futures::StreamExt;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::worker::{Cursor, InvocationRequest, UpdateMode};
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_server::WorkerExecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ActivatePluginRequest, ActivatePluginResponse, AppendToStreamSlotRequest,
    AppendToStreamSlotResponse, CancelInvocationRequest, CancelInvocationResponse,
    ConnectWorkerRequest, DeactivatePluginRequest, DeactivatePluginResponse, DeleteWorkerRequest,
    DeliverCardTransferRequest, DeliverCardTransferResponse, DurableStreamAttachmentControlRequest,
    DurableStreamAttachmentControlResponse, DurableStreamSegmentReadRequest,
    DurableStreamSegmentReadResponse, ForkStreamSlotRequest, ForkStreamSlotResponse,
    ForkWorkerRequest, ForkWorkerResponse, GetAgentWalletRequest, GetAgentWalletResponse,
    GetAgentWalletSuccess, GetFileContentsRequest, GetFileContentsResponse,
    GetFileSystemNodeRequest, GetFileSystemNodeResponse, GetOplogEntryRequest,
    GetOplogEntryResponse, GetOplogRequest, GetOplogResponse, GetRunningWorkersMetadataRequest,
    GetRunningWorkersMetadataResponse, GetWorkersMetadataRequest, GetWorkersMetadataResponse,
    ProcessOplogEntriesRequest, ProcessOplogEntriesResponse, ReadStreamSlotRequest,
    ReadStreamSlotResponse, ResolveRevertLastInvocationsRequest,
    ResolveRevertLastInvocationsResponse, RevertWorkerRequest, RevertWorkerResponse,
    SearchOplogRequest, SearchOplogResponse, UpdateWorkerRequest, UpdateWorkerResponse,
    deliver_card_transfer_response, durable_stream_attachment_control_response,
    durable_stream_segment_read_response, process_oplog_entries_response,
    resolve_revert_last_invocations_response,
};
use golem_common::base_model::durable_stream::{
    DurableStreamReadRequest, StreamAttachmentControlRequest,
};
use golem_common::metrics::api::{
    record_closed_grpc_api_active_stream, record_new_grpc_api_active_stream,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentConfigEntryDto, AgentMetadataDto, ResolvedRevert, TypedAgentConfigEntry,
};
use golem_common::model::agent::{
    AgentMode, InvocationFreshnessDisposition, ParsedAgentId, Principal,
};
use golem_common::model::card::{CardId, StoredCard, card_matches_agent_recipient};
use golem_common::model::component::{CanonicalFilePath, ComponentId, PluginPriority};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::filesystem::{FileByteSelection, FileReadError, validate_file_read_path};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::types::AgentMetadataForGuests;
use golem_common::model::oplog::{OplogErrorKind, OplogIndex};
use golem_common::model::protobuf::shard_epochs_from_proto;
use golem_common::model::protobuf::to_protobuf_resource_description;
use golem_common::model::{
    AgentEvent, AgentFilter, AgentFingerprint, AgentId, AgentInvocation, AgentMetadata,
    AgentStatus, AgentStatusRecord, IdempotencyKey, OwnedAgentId, ScanCursor, ShardDeliveryOutcome,
    ShardEpoch, ShardId, ShardLeaseRevision, Timestamp,
};
use golem_common::{model as common_model, recorded_grpc_api_request};
use golem_service_base::clients::shard_manager::ExecutorEndpoint;
use golem_service_base::error::worker_executor::*;
use golem_service_base::grpc::{
    proto_agent_id_string, proto_idempotency_key_string, proto_promise_id_string,
};
use golem_service_base::model::FileReadResponse;
use golem_service_base::model::GetFileSystemNodeResult;
use golem_service_base::model::auth::AuthCtx;
use prost::Message;
use std::cmp::min;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tonic::{Request, Response, Status};
use tracing::info_span;
use tracing::{Instrument, info};
use wasmtime::Error;

/// This is the implementation of the Worker Executor gRPC API
pub struct WorkerExecutorImpl<
    Ctx: WorkerCtx,
    Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static,
> {
    /// Reference to all the initialized services
    services: Svcs,
    /// Holds the strong Arc to the worker activator so the Weak reference
    /// stored in LazyWorkerActivator remains valid while the gRPC server runs.
    _worker_activator: Arc<dyn WorkerActivator<Ctx>>,
    /// Same arrangement for the assignment-changed hook: the shard manager service holds a Weak
    /// to it (see `GrpcShardManagerService::assignment_changed_hook`); this is the strong one.
    _assignment_changed_hook: ShardAssignmentChangedHook,
    /// Admits `GetOplogEntry` reads, each of which holds one full oplog entry in memory.
    oplog_entry_reads: Arc<Semaphore>,
    ctx: PhantomData<Ctx>,
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static> Clone
    for WorkerExecutorImpl<Ctx, Svcs>
{
    fn clone(&self) -> Self {
        Self {
            services: self.services.clone(),
            _worker_activator: self._worker_activator.clone(),
            _assignment_changed_hook: self._assignment_changed_hook.clone(),
            oplog_entry_reads: self.oplog_entry_reads.clone(),
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
        endpoint: ExecutorEndpoint,
    ) -> Result<Self, Error> {
        let worker_activator: Arc<dyn WorkerActivator<Ctx>> =
            Arc::new(DefaultWorkerActivator::new(services.clone()));

        lazy_worker_activator.set(worker_activator.clone());

        // A re-registration after `LeaseNotFound`, and a renewal that corrected the set, must
        // announce the new assignment exactly as this function and `assign_shards_internal` do.
        // The renewal loop cannot name `Ctx`, so it is handed this hook — installed before
        // `register`, which is what starts that loop.
        let hook_services = services.clone();
        let assignment_changed_hook: ShardAssignmentChangedHook = Arc::new(move || {
            let services = hook_services.clone();
            Box::pin(async move { Self::apply_shard_assignment_effects(&services).await })
        });

        let worker_executor = WorkerExecutorImpl {
            services: services.clone(),
            _worker_activator: worker_activator,
            _assignment_changed_hook: assignment_changed_hook.clone(),
            oplog_entry_reads: Arc::new(Semaphore::new(
                services.config().limits.max_concurrent_oplog_entry_reads,
            )),
            ctx: PhantomData,
        };

        info!(
            port = endpoint.port,
            advertised_ip = ?endpoint.advertised_ip,
            "Registering worker executor"
        );

        worker_executor
            .shard_manager_service()
            .set_assignment_changed_hook(&assignment_changed_hook);

        let pod_name = std::env::var_os("POD_NAME").map(|s| s.to_string_lossy().to_string());
        let shard_assignment = worker_executor
            .shard_manager_service()
            .register(endpoint, pod_name)
            .await?;

        info!(assignment = %shard_assignment, "Received initial shard assignment");

        match shard_assignment.revision {
            Some(revision) => worker_executor.shard_service().register(
                shard_assignment.number_of_shards,
                &shard_assignment.shard_epochs,
                shard_assignment.expires_at,
                revision,
            ),
            // The single-shard executor: no shard manager delivered this assignment, so there is
            // nothing to order it against.
            None => worker_executor.shard_service().install_unexpiring(
                shard_assignment.number_of_shards,
                &shard_assignment.shard_epochs,
            ),
        };

        // Deliberately fatal to startup, unlike the same failure on a running executor.
        //
        // This reads the running-worker recovery index, so a failure here means the executor does
        // not know which workers it is meant to resume. It refuses to start rather than serve with
        // an unknown recovery set, and the restart policy retries it - which costs nothing, because
        // an executor that has not started yet is holding no agents.
        //
        // That is the whole reason the same storage failure is *not* fatal once agents are running:
        // there, aborting would force every one of them to replay from oplog or snapshot, which is
        // far more expensive than stalling through a failover that resolves in tens of seconds.
        Self::apply_shard_assignment_effects(&worker_executor)
            .await
            .map_err(wasmtime::Error::from_anyhow)?;

        Ok(worker_executor)
    }

    /// The single ownership check behind every inbound `WorkerExecutor` RPC
    /// that names an agent. Every one of those is an ADMISSION decision — there
    /// is no remote fallback, rejecting is the only outcome — so this is where
    /// the self-fence goes.
    fn ensure_worker_belongs_to_this_executor(
        &self,
        agent_id: impl AsRef<AgentId>,
    ) -> Result<(), WorkerExecutorError> {
        self.shard_service().check_admission(agent_id.as_ref())
    }

    /// Rewrites the `OwnedAgentId` so that `environment_id` comes from the
    /// target component's metadata rather than from the caller-supplied
    /// request data. This ensures that shard routing, auth checks, and all
    /// downstream code use the component-authoritative environment.
    async fn canonicalize_owned_agent_id(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<OwnedAgentId, WorkerExecutorError> {
        let component = self
            .component_service()
            .get_metadata(owned_agent_id.component_id(), None)
            .await?;
        Ok(OwnedAgentId::new(
            component.environment_id,
            &owned_agent_id.agent_id,
        ))
    }

    /// Validate that the request carries a well-formed `auth_ctx` field.
    ///
    /// TODO: Now that `account_id` is resolved from the component owner
    /// (golemcloud/golem#3099), should we still require callers to send a
    /// valid `auth_ctx`? The value is no longer used for worker creation or
    /// resource attribution — but rejecting malformed requests early may
    /// still be desirable for protocol correctness?
    fn validate_auth_ctx(
        auth_ctx: &Option<golem_api_grpc::proto::golem::auth::AuthCtx>,
    ) -> Result<(), WorkerExecutorError> {
        let proto = auth_ctx
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?;
        let _: AuthCtx = proto.try_into().map_err(|e| {
            WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {e}"))
        })?;
        Ok(())
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

pub use events::WorkerEventStream;

fn extract_principal(
    proto_principal: &Option<golem_api_grpc::proto::golem::component::Principal>,
) -> Principal {
    proto_principal
        .as_ref()
        .and_then(|p| {
            let result: Result<Principal, String> = p.clone().try_into();
            result.ok()
        })
        .unwrap_or_else(Principal::anonymous)
}

fn extract_owned_agent_id<T>(
    request: &T,
    get_agent_id: impl FnOnce(&T) -> &Option<golem::worker::AgentId>,
    get_environment_id: impl FnOnce(&T) -> &Option<golem::common::EnvironmentId>,
) -> Result<OwnedAgentId, WorkerExecutorError> {
    let agent_id = get_agent_id(request)
        .as_ref()
        .ok_or(WorkerExecutorError::invalid_request("agent_id not found"))?;

    AgentId::validate_length(&agent_id.name).map_err(WorkerExecutorError::invalid_request)?;

    let agent_id = agent_id
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

    Ok(OwnedAgentId::new(environment_id, &agent_id))
}

mod agents;
mod api;
mod cards;
mod durable_stream;
mod events;
mod filesystem;
mod metadata;
mod oplog;
mod plugins;
mod shards;

#[cfg(test)]
mod tests;

use oplog::{OPLOG_ENTRY_CHUNK_SIZE, oplog_entry_frames};
