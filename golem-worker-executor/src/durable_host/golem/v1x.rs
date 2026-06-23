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

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::{
    ActiveAtomicRegion, DurabilityHost, DurableWorkerCtx, InternalRetryResult,
};
use crate::get_oplog_entry;
use crate::model::public_oplog::{
    PublicOplogEntryOps, find_component_revision_at, get_public_oplog_chunk, search_public_oplog,
};
use crate::preview2::golem_api_1_x;
use crate::preview2::golem_api_1_x::host::{
    AgentAnyFilter, ForkDetails, ForkResult, GetAgents, Host, HostGetAgents, HostGetPromiseResult,
    HostGetPromiseResultWithStore,
};
use crate::preview2::golem_api_1_x::oplog::{
    Host as OplogHost, HostGetOplog, HostSearchOplog, SearchOplog,
};
use crate::services::oplog::CommitLevel;
use crate::services::promise::{PromiseHandle, PromiseService};
use crate::services::worker_proxy::WorkerProxyError;
use crate::services::{HasOplogService, HasWorker};
use crate::worker::status::calculate_last_known_status_with_checkpoint;
use crate::workerctx::{StatusManagement, WorkerCtx};
use anyhow::anyhow;
use golem_common::model::agent::LegacyParsedAgentId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::oplog::host_functions::{
    GolemApiCompletePromise, GolemApiCreatePromise, GolemApiFork, GolemApiForkWorker,
    GolemApiGenerateIdempotencyKey, GolemApiGetAgentMetadata, GolemApiGetSelfMetadata,
    GolemApiResolveAgentIdStrict, GolemApiResolveComponentId, GolemApiRevertWorker,
    GolemApiUpdateWorker,
};
use golem_common::model::oplog::types::AgentMetadataForGuests;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemApiAgentId, HostRequestGolemApiComponentSlug,
    HostRequestGolemApiComponentSlugAndAgentName, HostRequestGolemApiForkAgent,
    HostRequestGolemApiPromiseId, HostRequestGolemApiRevertAgent, HostRequestGolemApiUpdateAgent,
    HostRequestNoInput, HostResponseGolemApiAgentId, HostResponseGolemApiAgentMetadata,
    HostResponseGolemApiComponentId, HostResponseGolemApiFork, HostResponseGolemApiIdempotencyKey,
    HostResponseGolemApiPromiseCompletion, HostResponseGolemApiPromiseId,
    HostResponseGolemApiSelfAgentMetadata, HostResponseGolemApiUnit, OplogEntry, PublicOplogEntry,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{AgentId, OwnedAgentId, ScanCursor};
use golem_common::model::{OplogIndex, PromiseId, RetryContext};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::debug;
use uuid::Uuid;
use wasmtime::component::{Accessor, HasSelf, Resource};
use wasmtime_wasi::IoView;

fn classify_worker_proxy_error(err: &WorkerProxyError) -> HostFailureKind {
    match err {
        WorkerProxyError::BadRequest(_)
        | WorkerProxyError::Unauthorized(_)
        | WorkerProxyError::LimitExceeded(_)
        | WorkerProxyError::NotFound(_)
        | WorkerProxyError::AlreadyExists(_) => HostFailureKind::Permanent,
        WorkerProxyError::InternalError(_) => HostFailureKind::Transient,
    }
}

fn classify_worker_executor_error(err: &WorkerExecutorError) -> HostFailureKind {
    match err {
        WorkerExecutorError::InvalidRequest { .. }
        | WorkerExecutorError::AgentAlreadyExists { .. }
        | WorkerExecutorError::AgentNotFound { .. }
        | WorkerExecutorError::PromiseNotFound { .. }
        | WorkerExecutorError::PromiseDropped { .. }
        | WorkerExecutorError::PromiseAlreadyCompleted { .. }
        | WorkerExecutorError::ParamTypeMismatch { .. }
        | WorkerExecutorError::NoValueInMessage
        | WorkerExecutorError::ValueMismatch { .. }
        | WorkerExecutorError::UnexpectedOplogEntry { .. }
        | WorkerExecutorError::InvalidAccount
        | WorkerExecutorError::PreviousInvocationFailed { .. }
        | WorkerExecutorError::PreviousInvocationExited
        | WorkerExecutorError::ComponentNotFound { .. } => HostFailureKind::Permanent,
        _ => HostFailureKind::Transient,
    }
}

impl<Ctx: WorkerCtx> HostGetAgents for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        component_id: golem_api_1_x::host::ComponentId,
        filter: Option<AgentAnyFilter>,
        precise: bool,
    ) -> anyhow::Result<Resource<GetAgents>> {
        self.observe_function_call("golem::api::get-workers", "new");
        let entry = GetAgentsEntry::new(
            component_id.into(),
            filter
                .map(|f| f.try_into())
                .transpose()
                .map_err(|e: String| anyhow!(e))?,
            precise,
        );
        let resource = self.as_wasi_view().table().push(entry)?;
        Ok(resource)
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetAgents>,
    ) -> anyhow::Result<Option<Vec<golem_api_1_x::host::AgentMetadata>>> {
        self.observe_function_call("golem::api::get-workers", "get-next");
        let (component_id, filter, count, precise, cursor) = self
            .as_wasi_view()
            .table()
            .get::<GetAgentsEntry>(&self_)
            .map(|e| {
                (
                    e.component_id,
                    e.filter.clone(),
                    e.count,
                    e.precise,
                    e.next_cursor.clone(),
                )
            })?;

        if let Some(cursor) = cursor {
            let (new_cursor, workers) = self
                .state
                .get_workers(&component_id, filter, cursor, count, precise)
                .await?;

            self.as_wasi_view()
                .table()
                .get_mut::<GetAgentsEntry>(&self_)
                .map(|e| e.set_next_cursor(new_cursor))?;

            Ok(Some(
                workers
                    .into_iter()
                    .map(|w| {
                        let metadata: AgentMetadataForGuests = w.into();
                        metadata.into()
                    })
                    .collect(),
            ))
        } else {
            Ok(None)
        }
    }

    async fn drop(&mut self, rep: Resource<GetAgents>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::get-workers", "drop");
        self.as_wasi_view().table().delete::<GetAgentsEntry>(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn create_promise(&mut self) -> anyhow::Result<golem_api_1_x::host::PromiseId> {
        let mut handle = CallHandle::<GolemApiCreatePromise, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::WriteLocal,
        )
        .await?;

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            // The promise oplog index is the host-call `Start` index: with the legacy atomic pair
            // this equalled `current_oplog_index().next()` captured before the pair was written.
            // It is stable across an incomplete-replay re-execution because the `Start` is reused.
            let oplog_idx = handle.start_index();
            let promise_id = self
                .public_state
                .promise_service
                .create(&self.owned_agent_id.agent_id, oplog_idx)
                .await;
            handle
                .complete(self, HostResponseGolemApiPromiseId { promise_id })
                .await?
        };

        Ok(result.promise_id.into())
    }

    async fn get_promise(
        &mut self,
        promise_id: golem_api_1_x::host::PromiseId,
    ) -> anyhow::Result<Resource<GetPromiseResultEntry>> {
        let entry =
            GetPromiseResultEntry::new(promise_id.into(), self.state.promise_service.clone());
        Ok(self.table().push(entry)?)
    }

    async fn complete_promise(
        &mut self,
        promise_id: golem_api_1_x::host::PromiseId,
        data: Vec<u8>,
    ) -> anyhow::Result<bool> {
        let promise_id: PromiseId = promise_id.into();

        let mut handle = CallHandle::<GolemApiCompletePromise, NotCancellable>::start(
            self,
            HostRequestGolemApiPromiseId {
                promise_id: promise_id.clone(),
            },
            DurableFunctionType::WriteLocal,
        )
        .await?;

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            // A promise must be completed on the instance that is owning the agent that originally created here.
            let agent_id = &promise_id.agent_id;

            let is_local_worker = match self.state.shard_service.check_worker(agent_id) {
                Ok(()) => true,
                Err(WorkerExecutorError::InvalidShardId { .. }) => false,
                Err(other) => {
                    handle.abandon_for_trap();
                    return Err(other.into());
                }
            };

            let promise_completion_result = if is_local_worker {
                match self
                    .public_state
                    .promise_service
                    .complete(promise_id.clone(), data)
                    .await
                {
                    Ok(completed) => completed,
                    Err(err) => {
                        handle.abandon_for_trap();
                        return Err(err.into());
                    }
                }
            } else {
                // talk to the executor that actually owns the promise
                match self
                    .state
                    .worker_proxy
                    .complete_promise(
                        promise_id.clone(),
                        data,
                        self.created_by(),
                        self.created_by_email(),
                    )
                    .await
                {
                    Ok(completed) => completed,
                    Err(err) => {
                        handle.abandon_for_trap();
                        return Err(err.into());
                    }
                }
            };

            handle
                .complete(
                    self,
                    HostResponseGolemApiPromiseCompletion {
                        completed: promise_completion_result,
                    },
                )
                .await?
        };

        Ok(result.completed)
    }

    async fn get_oplog_index(&mut self) -> anyhow::Result<golem_api_1_x::oplog::OplogIndex> {
        self.observe_function_call("golem::api", "get_oplog_index");
        if self.state.is_live() {
            self.state.oplog.add(OplogEntry::no_op()).await;
            let marker = self.state.current_oplog_index().await;
            // This `NoOp` index is the realistic `set_oplog_index` target; pin the mid-invocation
            // checkpoint watermark to the earliest one so a checkpoint at `<= marker` survives a
            // later jump back to it. (No checkpoint is taken here — there is no commit at this
            // point; the next post-commit boundary at/below the marker takes it.)
            self.state.min_exposed_marker = Some(match self.state.min_exposed_marker {
                Some(existing) => existing.min(marker),
                None => marker,
            });
            Ok(marker.into())
        } else {
            let (oplog_index, _) = get_oplog_entry!(self.state.replay_state, OplogEntry::NoOp)?;
            // The replayed `get_oplog_index` returns this same marker to the guest, which may feed
            // it to `set_oplog_index` after switching to live. Pin the watermark here too (mirroring
            // the live branch) so a post-replay mid-invocation checkpoint never advances past it.
            self.state.min_exposed_marker = Some(match self.state.min_exposed_marker {
                Some(existing) => existing.min(oplog_index),
                None => oplog_index,
            });
            Ok(oplog_index.into())
        }
    }

    async fn set_oplog_index(
        &mut self,
        oplog_idx: golem_api_1_x::oplog::OplogIndex,
    ) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "set_oplog_index");
        let jump_source = self.state.current_oplog_index().await.next(); // index of the Jump instruction that we will add
        let jump_target = OplogIndex::from_u64(oplog_idx).next(); // we want to jump _after_ reaching the target index
        let original_target = OplogIndex::from_u64(oplog_idx); // the actual oplog entry the user wants to jump to
        if jump_target > jump_source {
            Err(anyhow!(
                "Attempted to jump forward in oplog to index {jump_target} from {jump_source}"
            ))
        } else if self
            .state
            .replay_state
            .is_in_skipped_region(original_target)
            .await
        {
            Err(anyhow!(
                "Attempted to jump to a deleted region in oplog to index {original_target} from {jump_source}"
            ))
        } else if self.state.is_live() {
            let jump = OplogRegion {
                start: jump_target,
                end: jump_source,
            };

            // Write an oplog entry with the new jump and then restart the worker
            self.public_state
                .worker()
                .add_and_commit_oplog(OplogEntry::jump(jump))
                .await;

            debug!("Interrupting live execution for jumping from {jump_source} to {jump_target}",);
            Err(InterruptKind::Jump.into())
        } else {
            // In replay mode we never have to do anything here
            debug!("Ignoring replayed set_oplog_index");
            Ok(())
        }
    }

    async fn oplog_commit(&mut self, replicas: u8) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "oplog_commit");
        if self.state.is_live() {
            let timeout = Duration::from_secs(1);
            debug!("Worker committing oplog to {replicas} replicas");
            loop {
                // Applying a timeout to make sure the worker remains interruptible
                if self.state.oplog.wait_for_replicas(replicas, timeout).await {
                    debug!("Worker committed oplog to {replicas} replicas");
                    return Ok(());
                } else {
                    debug!("Worker failed to commit oplog to {replicas} replicas, retrying",);
                }

                if let Some(kind) = self.check_interrupt() {
                    return Err(kind.into());
                }
            }
        } else {
            Ok(())
        }
    }

    async fn mark_begin_operation(&mut self) -> anyhow::Result<golem_api_1_x::host::OplogIndex> {
        self.observe_function_call("golem::api", "mark_begin_operation");

        if self.state.is_live() {
            let next_idempotency_key_oplog_index = self
                .state
                .current_atomic_region_idempotency_key_oplog_index();
            self.state
                .oplog
                .add(OplogEntry::begin_atomic_region())
                .await;
            let begin_index = self.state.current_oplog_index().await;
            let next_idempotency_key_oplog_index =
                next_idempotency_key_oplog_index.unwrap_or_else(|| begin_index.next());
            self.state.active_atomic_regions.push(ActiveAtomicRegion {
                begin_index,
                next_idempotency_key_oplog_index,
                has_side_effects: false,
                in_flight_call_count: 0,
            });
            Ok(begin_index.into())
        } else {
            let (begin_index, _) =
                get_oplog_entry!(self.state.replay_state, OplogEntry::BeginAtomicRegion)?;

            match self
                .state
                .replay_state
                .lookup_oplog_entry(begin_index, OplogEntry::is_end_atomic_region)
                .await
            {
                Some(end_index) => {
                    debug!(
                        "Worker's atomic operation starting at {} is already committed at {}",
                        begin_index, end_index
                    );
                }
                None => {
                    debug!(
                        "Worker's atomic operation starting at {} is not committed, ignoring persisted entries",
                        begin_index
                    );

                    // We need to jump to the end of the oplog
                    self.state.replay_state.switch_to_live().await;

                    // But this is not enough, because if the retried transactional block succeeds,
                    // and later we replay it, we need to skip the first attempt and only replay the second.
                    // Se we add a Jump entry to the oplog that registers a deleted region.
                    let deleted_region = OplogRegion {
                        start: begin_index.next(), // need to keep the BeginAtomicRegion entry
                        end: self.state.replay_state.replay_target().next(), // skipping the Jump entry too
                    };

                    self.public_state
                        .worker()
                        .add_and_commit_oplog(OplogEntry::jump(deleted_region))
                        .await;

                    // TODO: this recomputation should not be necessary.
                    self.public_state.worker().reattach_worker_status().await;
                }
            }

            self.state.active_atomic_regions.push(ActiveAtomicRegion {
                begin_index,
                next_idempotency_key_oplog_index: begin_index.next(),
                has_side_effects: false,
                in_flight_call_count: 0,
            });
            Ok(begin_index.into())
        }
    }

    async fn mark_end_operation(
        &mut self,
        begin: golem_api_1_x::oplog::OplogIndex,
    ) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "mark_end_operation");
        let begin_index = OplogIndex::from_u64(begin);
        if self.state.is_live() {
            if self.state.atomic_region_has_in_flight_calls(begin_index) {
                return Err(anyhow::anyhow!(
                    "cannot end atomic region {begin_index} while durable calls initiated in it are still in flight"
                ));
            }
            self.state
                .oplog
                .add(OplogEntry::end_atomic_region(begin_index))
                .await;
        } else {
            let (_, _) = get_oplog_entry!(self.state.replay_state, OplogEntry::EndAtomicRegion)?;
        }

        self.state
            .active_atomic_regions
            .retain(|region| region.begin_index != begin_index);

        Ok(())
    }

    async fn trap(&mut self, reason: String) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "trap");
        Err(anyhow::anyhow!("guest-requested trap: {reason}"))
    }

    async fn get_oplog_persistence_level(
        &mut self,
    ) -> anyhow::Result<golem_api_1_x::host::PersistenceLevel> {
        self.observe_function_call("golem::api", "get_oplog_persistence_level");
        Ok(self.state.persistence_level.into())
    }

    async fn set_oplog_persistence_level(
        &mut self,
        new_persistence_level: golem_api_1_x::host::PersistenceLevel,
    ) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "set_oplog_persistence_level");

        let new_persistence_level = new_persistence_level.into();
        if self.state.persistence_level != new_persistence_level {
            // commit all pending entries and change persistence level
            if self.state.is_live() {
                self.public_state
                    .worker()
                    .add_and_commit_oplog(OplogEntry::change_persistence_level(
                        new_persistence_level,
                    ))
                    .await;
            } else {
                let oplog_index_before = self.state.current_oplog_index().await;
                let (_, _) =
                    get_oplog_entry!(self.state.replay_state, OplogEntry::ChangePersistenceLevel)?;
                let oplog_index_after = self.state.current_oplog_index().await;
                if self.state.replay_state.is_live()
                    && oplog_index_after > oplog_index_before.next()
                {
                    // get_oplog_entry jumped to live mode because the persist-nothing zone was not closed.
                    // If the retried transactional block succeeds, and later we replay it, we need to skip the first
                    // attempt and only replay the second.
                    // Se we add a Jump entry to the oplog that registers a deleted region.
                    let deleted_region = OplogRegion {
                        start: oplog_index_before.next(), // need to keep the BeginAtomicRegion entry
                        end: self.state.replay_state.replay_target().next(), // skipping the Jump entry too
                    };

                    self.public_state
                        .worker()
                        .add_and_commit_oplog(OplogEntry::jump(deleted_region))
                        .await;

                    // TODO: this recomputation should not be necessary.
                    self.public_state.worker().reattach_worker_status().await;
                }
            }

            self.state
                .oplog
                .switch_persistence_level(new_persistence_level)
                .await;
            self.state.persistence_level = new_persistence_level;
            debug!(
                "Worker's oplog persistence level is set to {:?}",
                self.state.persistence_level
            );
        }
        Ok(())
    }

    async fn get_idempotence_mode(&mut self) -> anyhow::Result<bool> {
        self.observe_function_call("golem::api", "get_idempotence_mode");
        Ok(self.state.assume_idempotence)
    }

    async fn set_idempotence_mode(&mut self, idempotent: bool) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "set_idempotence_mode");
        self.state.assume_idempotence = idempotent;
        Ok(())
    }

    async fn generate_idempotency_key(&mut self) -> anyhow::Result<golem_api_1_x::host::Uuid> {
        let handle = CallHandle::<GolemApiGenerateIdempotencyKey, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::WriteRemote,
        )
        .await?;

        // Even though `IdempotencyKey::derived` is used, we still need to persist this, because the
        // derived key depends on the oplog index. `begin_index()` is the durable-scope index of this
        // call; reusing it (rather than reading the live oplog index again) keeps the derived key
        // stable across an incomplete-replay re-execution, since the `Start` is reused.
        let oplog_index = handle.begin_index();

        let result = handle
            .run(self, async |ctx| {
                let key = ctx.derive_idempotency_key(oplog_index);
                let uuid = Uuid::parse_str(&key.value.to_string()).unwrap(); // this is guaranteed to be an uuid
                Ok::<_, anyhow::Error>(HostResponseGolemApiIdempotencyKey { uuid })
            })
            .await?;
        Ok(result.uuid.into())
    }

    async fn update_agent(
        &mut self,
        agent_id: golem_api_1_x::host::AgentId,
        target_version: u64,
        mode: golem_api_1_x::host::UpdateMode,
    ) -> anyhow::Result<()> {
        // NOTE: Mode-changing updates are rejected by the target worker-executor's gRPC
        // `update_worker` handler. The error returned by `worker_proxy.update` below
        // propagates back to the caller agent as a regular update error, so no additional
        // local check is needed here.
        let agent_id: AgentId = agent_id.into();
        let owned_agent_id = OwnedAgentId::new(self.owned_agent_id.environment_id, &agent_id);

        let mode = match mode {
            golem_api_1_x::host::UpdateMode::Automatic => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Automatic
            }
            golem_api_1_x::host::UpdateMode::SnapshotBased => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Manual
            }
        };

        let target_revision: ComponentRevision =
            target_version.try_into().map_err(|e: String| anyhow!(e))?;

        let mut handle = CallHandle::<GolemApiUpdateWorker, NotCancellable>::start(
            self,
            HostRequestGolemApiUpdateAgent {
                agent_id,
                target_revision,
                mode,
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("update-agent");
            let result = loop {
                let result = self
                    .state
                    .worker_proxy
                    .update(
                        &owned_agent_id,
                        target_revision,
                        mode,
                        false,
                        self.created_by(),
                        self.created_by_email(),
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_worker_proxy_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.map_err(|err| err.to_string());
            handle
                .complete(self, HostResponseGolemApiUnit { result })
                .await?
        };

        result.result.map_err(|err| anyhow!(err))
    }

    async fn get_self_metadata(&mut self) -> anyhow::Result<golem_api_1_x::host::AgentMetadata> {
        let handle = CallHandle::<GolemApiGetSelfMetadata, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(self, async |ctx| {
                let metadata = ctx
                    .public_state
                    .worker()
                    .get_latest_worker_metadata()
                    .await
                    .into();
                Ok::<_, anyhow::Error>(HostResponseGolemApiSelfAgentMetadata { metadata })
            })
            .await?;

        Ok(result.metadata.into())
    }

    async fn get_agent_metadata(
        &mut self,
        agent_id: golem_api_1_x::host::AgentId,
    ) -> anyhow::Result<Option<golem_api_1_x::host::AgentMetadata>> {
        let agent_id: AgentId = agent_id.into();

        let handle = CallHandle::<GolemApiGetAgentMetadata, NotCancellable>::start(
            self,
            HostRequestGolemApiAgentId {
                agent_id: agent_id.clone(),
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = handle
            .run(self, async |ctx| {
                let owned_agent_id =
                    OwnedAgentId::new(ctx.owned_agent_id.environment_id, &agent_id);
                let result = ctx.state.worker_service.get(&owned_agent_id).await;
                let metadata: Option<AgentMetadataForGuests> = if let Some(result) = result {
                    let mut metadata = result.initial_worker_metadata;
                    if let Some(last_known_status) = &result.last_known_status {
                        metadata.last_known_status = last_known_status.clone();
                    }
                    let agent_mode = metadata.agent_mode;
                    if let Some(status) = calculate_last_known_status_with_checkpoint(
                        &ctx.state,
                        &owned_agent_id,
                        agent_mode,
                        result.last_known_status,
                    )
                    .await
                    {
                        metadata.last_known_status = status;
                    }
                    Some(metadata.into())
                } else {
                    None
                };
                Ok::<_, anyhow::Error>(HostResponseGolemApiAgentMetadata { metadata })
            })
            .await?;

        Ok(result.metadata.map(|metadata| metadata.into()))
    }

    async fn fork_agent(
        &mut self,
        source_agent_id: golem_api_1_x::host::AgentId,
        target_agent_id: golem_api_1_x::host::AgentId,
        oplog_idx_cut_off: golem_api_1_x::host::OplogIndex,
    ) -> anyhow::Result<()> {
        let source_agent_id: AgentId = source_agent_id.into();
        let target_agent_id: AgentId = target_agent_id.into();

        let oplog_index_cut_off: OplogIndex = OplogIndex::from_u64(oplog_idx_cut_off);

        let mut handle = CallHandle::<GolemApiForkWorker, NotCancellable>::start(
            self,
            HostRequestGolemApiForkAgent {
                source_agent_id: source_agent_id.clone(),
                target_agent_id: target_agent_id.clone(),
                oplog_index_cut_off,
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("fork-agent");
            let result = loop {
                let result = self
                    .state
                    .worker_proxy
                    .fork_worker(
                        &source_agent_id,
                        &target_agent_id,
                        &oplog_index_cut_off,
                        self.created_by(),
                        self.created_by_email(),
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_worker_proxy_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.map_err(|err| err.to_string());
            handle
                .complete(self, HostResponseGolemApiUnit { result })
                .await?
        };

        result.result.map_err(|err| anyhow!(err))
    }

    async fn revert_agent(
        &mut self,
        agent_id: golem_api_1_x::host::AgentId,
        revert_target: golem_api_1_x::host::RevertAgentTarget,
    ) -> anyhow::Result<()> {
        let agent_id: AgentId = agent_id.into();
        let target: golem_common::model::worker::RevertWorkerTarget = revert_target.into();

        let mut handle = CallHandle::<GolemApiRevertWorker, NotCancellable>::start(
            self,
            HostRequestGolemApiRevertAgent {
                agent_id: agent_id.clone(),
                target: target.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("revert-agent");
            let result = loop {
                let result = self
                    .worker_proxy()
                    .revert(
                        &agent_id,
                        target.clone(),
                        self.created_by(),
                        self.created_by_email(),
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_worker_proxy_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.map_err(|err| err.to_string());
            handle
                .complete(self, HostResponseGolemApiUnit { result })
                .await?
        };

        result.result.map_err(|err| anyhow!(err))
    }

    async fn resolve_component_id(
        &mut self,
        component_slug: String,
    ) -> anyhow::Result<Option<golem_api_1_x::host::ComponentId>> {
        let mut handle = CallHandle::<GolemApiResolveComponentId, NotCancellable>::start(
            self,
            HostRequestGolemApiComponentSlug {
                component_slug: component_slug.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("resolve-component-id");
            let result = loop {
                let result = self
                    .state
                    .component_service
                    .resolve_component(
                        component_slug.clone(),
                        self.state.component_metadata.environment_id,
                        self.state.component_metadata.application_id,
                        self.state.component_metadata.account_id,
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_worker_executor_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.map_err(|err| err.to_string());
            handle
                .complete(self, HostResponseGolemApiComponentId { result })
                .await?
        };

        result
            .result
            .map(|opt| opt.map(golem_api_1_x::host::ComponentId::from))
            .map_err(|err| anyhow!(err))
    }

    async fn resolve_agent_id(
        &mut self,
        component_slug: String,
        agent_id: String,
    ) -> anyhow::Result<Option<golem_api_1_x::host::AgentId>> {
        let component_id = self.resolve_component_id(component_slug).await?;
        Ok(
            component_id.map(|component_id| golem_api_1_x::host::AgentId {
                component_id,
                agent_id,
            }),
        )
    }

    async fn resolve_agent_id_strict(
        &mut self,
        component_slug: String,
        agent_name: String,
    ) -> anyhow::Result<Option<golem_api_1_x::host::AgentId>> {
        let mut handle = CallHandle::<GolemApiResolveAgentIdStrict, NotCancellable>::start(
            self,
            HostRequestGolemApiComponentSlugAndAgentName {
                component_slug: component_slug.clone(),
                agent_name: agent_name.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("resolve-agent-id-strict");
            let result = loop {
                let result = self
                    .resolve_agent_id_strict_internal(component_slug.clone(), agent_name.clone())
                    .await;

                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &result,
                        classify_worker_executor_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.map_err(|err| err.to_string());
            handle
                .complete(self, HostResponseGolemApiAgentId { result })
                .await?
        };

        result
            .result
            .map(|opt| opt.map(golem_api_1_x::host::AgentId::from))
            .map_err(|err| anyhow!(err))
    }

    async fn fork(&mut self) -> anyhow::Result<ForkResult> {
        let mut handle = CallHandle::<GolemApiFork, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let retry_properties = RetryContext::golem_api("fork");
            let forked_phantom_id = Uuid::new_v4();

            let new_name = if let Some(agent_id) = self.parsed_agent_id() {
                match LegacyParsedAgentId::new(
                    agent_id.agent_type.clone(),
                    agent_id.parameters.clone(),
                    Some(forked_phantom_id),
                ) {
                    Ok(parsed) => parsed.to_string(),
                    Err(err) => {
                        handle.abandon_for_trap();
                        return Err(anyhow!(err));
                    }
                }
            } else {
                format!("{}-{}", self.agent_id().agent_id, forked_phantom_id)
            };

            let target_agent_id = AgentId {
                component_id: self.owned_agent_id.component_id(),
                agent_id: new_name.clone(),
            };
            // The forked worker must inherit the source state from the source-side fork call's
            // durable replay point. In non-idempotent mode this is the outer `WriteRemote` scope
            // `Start`; `fork_and_write_fork_result` completes that copied scope around a synthetic
            // child call carrying `ForkResult::Forked`. The source call completes later with
            // `ForkResult::Original`. We still force a commit so the eager `Start` is durable for
            // this (source) worker's own crash recovery.
            let oplog_index_cut_off = handle.begin_index();
            let copied_scope_start = self
                .state
                .opens_durable_scope(&DurableFunctionType::WriteRemote)
                .then_some(oplog_index_cut_off);
            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;

            let created_by = self.created_by();
            let created_by_email = self.created_by_email().clone();
            let fork_result = loop {
                let fork_result = self
                    .state
                    .worker_fork
                    .fork_and_write_fork_result(
                        created_by,
                        &created_by_email,
                        &self.owned_agent_id,
                        &target_agent_id,
                        oplog_index_cut_off,
                        copied_scope_start,
                        forked_phantom_id,
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop_with_properties(
                        self,
                        &fork_result,
                        classify_worker_executor_error,
                        retry_properties.clone(),
                    )
                    .await?
                {
                    InternalRetryResult::Persist => break fork_result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            let fork_result = fork_result
                .map(|_| golem_common::model::ForkResult::Original)
                .map_err(|err| err.to_string());
            handle
                .complete(
                    self,
                    HostResponseGolemApiFork {
                        forked_phantom_id,
                        result: fork_result,
                    },
                )
                .await?
        };

        match result.result {
            Ok(fork_result) => {
                let details = ForkDetails {
                    forked_phantom_id: result.forked_phantom_id.into(),
                };
                Ok(match fork_result {
                    golem_common::model::ForkResult::Original => ForkResult::Original(details),
                    golem_common::model::ForkResult::Forked => ForkResult::Forked(details),
                })
            }
            Err(err) => Err(anyhow!(err)),
        }
    }
}

impl<Ctx: WorkerCtx> HostGetOplog for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        agent_id: golem_api_1_x::oplog::AgentId,
        start: golem_api_1_x::oplog::OplogIndex,
    ) -> anyhow::Result<Resource<GetOplogEntry>> {
        self.observe_function_call("golem::api::get-oplog", "new");

        let agent_id: AgentId = agent_id.into();
        let owned_agent_id = OwnedAgentId::new(self.owned_agent_id.environment_id(), &agent_id);

        let start = OplogIndex::from_u64(start);
        let agent_mode = self
            .state
            .worker_service
            .get_agent_mode(&owned_agent_id)
            .await
            .ok_or_else(|| anyhow!("agent {} does not exist", owned_agent_id))?;
        let initial_component_version = find_component_revision_at(
            self.state.oplog_service(),
            &owned_agent_id,
            agent_mode,
            start,
        )
        .await?;

        let entry = GetOplogEntry::new(owned_agent_id, start, initial_component_version, 100);
        let resource = self.as_wasi_view().table().push(entry)?;
        Ok(resource)
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetOplogEntry>,
    ) -> anyhow::Result<Option<Vec<golem_api_1_x::oplog::PublicOplogEntry>>> {
        self.observe_function_call("golem::api::get-oplog", "get-next");

        let entry = self.as_wasi_view().table().get(&self_)?.clone();
        let agent_type =
            LegacyParsedAgentId::parse_agent_type_name(&entry.owned_agent_id.agent_id.agent_id)
                .ok();
        let component_service = self.state.component_service.clone();
        let oplog_service = self.state.oplog_service();

        let agent_mode = self
            .state
            .worker_service
            .get_agent_mode(&entry.owned_agent_id)
            .await
            .ok_or_else(|| anyhow!("agent {} does not exist", entry.owned_agent_id))?;

        let chunk = get_public_oplog_chunk(
            component_service,
            oplog_service,
            &entry.owned_agent_id,
            agent_mode,
            agent_type.as_ref(),
            entry.current_component_revision,
            entry.next_oplog_index,
            entry.page_size,
        )
        .await
        .map_err(|msg| anyhow!(msg))?;

        if chunk.next_oplog_index != entry.next_oplog_index {
            self.as_wasi_view()
                .table()
                .get_mut(&self_)?
                .update(chunk.next_oplog_index, chunk.current_component_revision);
            Ok(Some(
                chunk
                    .entries
                    .into_iter()
                    .map(|entry| entry.into())
                    .collect(),
            ))
        } else {
            Ok(None)
        }
    }

    async fn drop(&mut self, rep: Resource<GetOplogEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::get-oplog", "drop");
        self.as_wasi_view().table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostGetPromiseResult for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, resource: Resource<GetPromiseResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::promise-result", "drop");
        let resource_rep = resource.rep();
        let _ = self.table().delete(resource)?;

        // This is optional because the entry is only created if we actually turn the GetPromiseResultEntry into a Pollable
        let dyn_pollable_reps = self
            .state
            .promise_dyn_pollables
            .write()
            .await
            .remove(&resource_rep);

        if let Some(set) = dyn_pollable_reps {
            for dyn_pollable_rep in set {
                let _ = self
                    .state
                    .promise_backed_pollables
                    .write()
                    .await
                    .remove(&dyn_pollable_rep);
            }
        };

        self.state.promise_service.cleanup().await;

        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostGetPromiseResultWithStore for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn get<T: Send>(
        _accessor: &Accessor<T, Self>,
        _resource: Resource<GetPromiseResultEntry>,
    ) -> anyhow::Result<Vec<u8>> {
        // TODO(p3): port the durable get-promise-result implementation to the
        // `Accessor`-based async pattern. The previous `&mut self` based logic
        // (which used `Durability::<GolemApiGetPromiseResult>::new`,
        // `entry.get_handle().await.get().await`, and `durability.persist(...)`)
        // does not translate directly because the new bindgen `get` lives on
        // `HostGetPromiseResultWithStore` and only exposes an `Accessor` rather
        // than `&mut self`.
        unimplemented!("HostGetPromiseResultWithStore::get (p3 migration)")
    }
}

#[derive(Debug, Clone)]
pub struct GetOplogEntry {
    pub owned_agent_id: OwnedAgentId,
    pub next_oplog_index: OplogIndex,
    pub current_component_revision: ComponentRevision,
    pub page_size: usize,
}

impl GetOplogEntry {
    pub fn new(
        owned_agent_id: OwnedAgentId,
        initial_oplog_index: OplogIndex,
        initial_component_revision: ComponentRevision,
        page_size: usize,
    ) -> Self {
        Self {
            owned_agent_id,
            next_oplog_index: initial_oplog_index,
            current_component_revision: initial_component_revision,
            page_size,
        }
    }

    pub fn update(
        &mut self,
        next_oplog_index: OplogIndex,
        current_component_revision: ComponentRevision,
    ) {
        self.next_oplog_index = next_oplog_index;
        self.current_component_revision = current_component_revision;
    }
}

impl<Ctx: WorkerCtx> HostSearchOplog for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        agent_id: golem_api_1_x::oplog::AgentId,
        text: String,
    ) -> anyhow::Result<Resource<SearchOplog>> {
        self.observe_function_call("golem::api::search-oplog", "new");

        let agent_id: AgentId = agent_id.into();
        let owned_agent_id = OwnedAgentId::new(self.owned_agent_id.environment_id(), &agent_id);

        let start = OplogIndex::INITIAL;
        let agent_mode = self
            .state
            .worker_service
            .get_agent_mode(&owned_agent_id)
            .await
            .ok_or_else(|| anyhow!("agent {} does not exist", owned_agent_id))?;
        let initial_component_version = find_component_revision_at(
            self.state.oplog_service(),
            &owned_agent_id,
            agent_mode,
            start,
        )
        .await?;

        let entry =
            SearchOplogEntry::new(owned_agent_id, start, initial_component_version, 100, text);
        let resource = self.as_wasi_view().table().push(entry)?;
        Ok(resource)
    }

    async fn get_next(
        &mut self,
        self_: Resource<SearchOplog>,
    ) -> anyhow::Result<
        Option<
            Vec<(
                golem_api_1_x::oplog::OplogIndex,
                golem_api_1_x::oplog::PublicOplogEntry,
            )>,
        >,
    > {
        self.observe_function_call("golem::api::search-oplog", "get-next");

        let entry = self.as_wasi_view().table().get(&self_)?.clone();
        let agent_type =
            LegacyParsedAgentId::parse_agent_type_name(&entry.owned_agent_id.agent_id.agent_id)
                .ok();
        let component_service = self.state.component_service.clone();
        let oplog_service = self.state.oplog_service();

        let agent_mode = self
            .state
            .worker_service
            .get_agent_mode(&entry.owned_agent_id)
            .await
            .ok_or_else(|| anyhow!("agent {} does not exist", entry.owned_agent_id))?;

        let chunk = search_public_oplog(
            component_service,
            oplog_service,
            &entry.owned_agent_id,
            agent_mode,
            agent_type.as_ref(),
            entry.current_component_revision,
            entry.next_oplog_index,
            entry.page_size,
            &entry.query,
        )
        .await
        .map_err(|msg| anyhow!(msg))?;

        if chunk.next_oplog_index != entry.next_oplog_index {
            self.as_wasi_view()
                .table()
                .get_mut(&self_)?
                .update(chunk.next_oplog_index, chunk.current_component_revision);
            Ok(Some(
                chunk
                    .entries
                    .into_iter()
                    .map(|(idx, entry)| {
                        let idx: golem_api_1_x::oplog::OplogIndex = idx.into();
                        let entry: golem_api_1_x::oplog::PublicOplogEntry = entry.into();
                        (idx, entry)
                    })
                    .collect(),
            ))
        } else {
            Ok(None)
        }
    }

    async fn drop(&mut self, rep: Resource<SearchOplog>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::search-oplog", "drop");
        self.as_wasi_view().table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    async fn resolve_agent_id_strict_internal(
        &self,
        component_slug: String,
        agent_name: String,
    ) -> Result<Option<AgentId>, WorkerExecutorError> {
        let component_id = self
            .state
            .component_service
            .resolve_component(
                component_slug.clone(),
                self.state.component_metadata.environment_id,
                self.state.component_metadata.application_id,
                self.state.component_metadata.account_id,
            )
            .await?;

        let agent_id = component_id.map(|component_id| AgentId {
            component_id,
            agent_id: agent_name.clone(),
        });

        if let Some(agent_id) = agent_id.clone() {
            let owned_id = OwnedAgentId {
                environment_id: self.state.owned_agent_id.environment_id(),
                agent_id,
            };

            let metadata = self.state.worker_service.get(&owned_id).await;

            if metadata.is_none() {
                return Ok(None);
            };
        };
        Ok(agent_id)
    }
}

#[derive(Debug, Clone)]
pub struct SearchOplogEntry {
    pub owned_agent_id: OwnedAgentId,
    pub next_oplog_index: OplogIndex,
    pub current_component_revision: ComponentRevision,
    pub page_size: usize,
    pub query: String,
}

impl SearchOplogEntry {
    pub fn new(
        owned_agent_id: OwnedAgentId,
        initial_oplog_index: OplogIndex,
        initial_component_revision: ComponentRevision,
        page_size: usize,
        query: String,
    ) -> Self {
        Self {
            owned_agent_id,
            next_oplog_index: initial_oplog_index,
            current_component_revision: initial_component_revision,
            page_size,
            query,
        }
    }

    pub fn update(
        &mut self,
        next_oplog_index: OplogIndex,
        current_component_revision: ComponentRevision,
    ) {
        self.next_oplog_index = next_oplog_index;
        self.current_component_revision = current_component_revision;
    }
}

impl<Ctx: WorkerCtx> OplogHost for DurableWorkerCtx<Ctx> {
    async fn enrich_oplog_entries(
        &mut self,
        environment_id: golem_api_1_x::host::EnvironmentId,
        agent_id: golem_api_1_x::oplog::AgentId,
        entries: Vec<(u64, golem_api_1_x::oplog::OplogEntry)>,
        component_revision: u64,
    ) -> anyhow::Result<Result<Vec<golem_api_1_x::oplog::PublicOplogEntry>, String>> {
        self.observe_function_call("golem::api::oplog", "enrich-oplog-entries");

        let component_service = self.state.component_service.clone();
        let oplog_service = self.state.oplog_service();
        let environment_id = golem_common::model::environment::EnvironmentId::from(
            Uuid::from_u64_pair(environment_id.uuid.high_bits, environment_id.uuid.low_bits),
        );
        let agent_id: AgentId = agent_id.into();
        let agent_type = LegacyParsedAgentId::parse_agent_type_name(&agent_id.agent_id).ok();
        let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);

        let mut current_revision = match ComponentRevision::try_from(component_revision) {
            Ok(rev) => rev,
            Err(e) => return Ok(Err(e.to_string())),
        };

        let agent_mode = match self
            .state
            .worker_service
            .get_agent_mode(&owned_agent_id)
            .await
        {
            Some(mode) => mode,
            None => return Ok(Err(format!("agent {owned_agent_id} does not exist"))),
        };

        let mut result = Vec::with_capacity(entries.len());
        for (index, wit_entry) in entries {
            let entry = match OplogEntry::try_from(wit_entry) {
                Ok(e) => e,
                Err(e) => return Ok(Err(e)),
            };

            if let Some(rev) = entry.specifies_component_revision() {
                current_revision = rev;
            }

            let oplog_index = OplogIndex::from_u64(index);
            match PublicOplogEntry::from_oplog_entry(
                oplog_index,
                entry,
                oplog_service.clone(),
                component_service.clone(),
                &owned_agent_id,
                agent_mode,
                agent_type.as_ref(),
                current_revision,
            )
            .await
            {
                Ok(public_entry) => {
                    let wit_entry: golem_api_1_x::oplog::PublicOplogEntry = public_entry.into();
                    result.push(wit_entry);
                }
                Err(e) => return Ok(Err(e)),
            }
        }

        Ok(Ok(result))
    }
}

impl From<golem_api_1_x::host::RevertAgentTarget>
    for golem_common::model::worker::RevertWorkerTarget
{
    fn from(value: golem_api_1_x::host::RevertAgentTarget) -> Self {
        match value {
            golem_api_1_x::host::RevertAgentTarget::RevertToOplogIndex(index) => {
                golem_common::model::worker::RevertWorkerTarget::RevertToOplogIndex(
                    golem_common::model::worker::RevertToOplogIndex {
                        last_oplog_index: OplogIndex::from_u64(index),
                    },
                )
            }
            golem_api_1_x::host::RevertAgentTarget::RevertLastInvocations(n) => {
                golem_common::model::worker::RevertWorkerTarget::RevertLastInvocations(
                    golem_common::model::worker::RevertLastInvocations {
                        number_of_invocations: n,
                    },
                )
            }
        }
    }
}

impl From<golem_api_1_x::host::FilterComparator> for golem_common::model::FilterComparator {
    fn from(value: golem_api_1_x::host::FilterComparator) -> Self {
        match value {
            golem_api_1_x::host::FilterComparator::Equal => {
                golem_common::model::FilterComparator::Equal
            }
            golem_api_1_x::host::FilterComparator::NotEqual => {
                golem_common::model::FilterComparator::NotEqual
            }
            golem_api_1_x::host::FilterComparator::Less => {
                golem_common::model::FilterComparator::Less
            }
            golem_api_1_x::host::FilterComparator::LessEqual => {
                golem_common::model::FilterComparator::LessEqual
            }
            golem_api_1_x::host::FilterComparator::Greater => {
                golem_common::model::FilterComparator::Greater
            }
            golem_api_1_x::host::FilterComparator::GreaterEqual => {
                golem_common::model::FilterComparator::GreaterEqual
            }
        }
    }
}

impl From<golem_api_1_x::host::StringFilterComparator>
    for golem_common::model::StringFilterComparator
{
    fn from(value: golem_api_1_x::host::StringFilterComparator) -> Self {
        match value {
            golem_api_1_x::host::StringFilterComparator::Equal => {
                golem_common::model::StringFilterComparator::Equal
            }
            golem_api_1_x::host::StringFilterComparator::NotEqual => {
                golem_common::model::StringFilterComparator::NotEqual
            }
            golem_api_1_x::host::StringFilterComparator::Like => {
                golem_common::model::StringFilterComparator::Like
            }
            golem_api_1_x::host::StringFilterComparator::NotLike => {
                golem_common::model::StringFilterComparator::NotLike
            }
            golem_api_1_x::host::StringFilterComparator::StartsWith => {
                golem_common::model::StringFilterComparator::StartsWith
            }
        }
    }
}

impl From<golem_api_1_x::host::AgentStatus> for golem_common::model::AgentStatus {
    fn from(value: golem_api_1_x::host::AgentStatus) -> Self {
        match value {
            golem_api_1_x::host::AgentStatus::Running => golem_common::model::AgentStatus::Running,
            golem_api_1_x::host::AgentStatus::Idle => golem_common::model::AgentStatus::Idle,
            golem_api_1_x::host::AgentStatus::Suspended => {
                golem_common::model::AgentStatus::Suspended
            }
            golem_api_1_x::host::AgentStatus::Interrupted => {
                golem_common::model::AgentStatus::Interrupted
            }
            golem_api_1_x::host::AgentStatus::Retrying => {
                golem_common::model::AgentStatus::Retrying
            }
            golem_api_1_x::host::AgentStatus::Failed => golem_common::model::AgentStatus::Failed,
            golem_api_1_x::host::AgentStatus::Exited => golem_common::model::AgentStatus::Exited,
        }
    }
}

impl From<golem_common::model::AgentStatus> for golem_api_1_x::host::AgentStatus {
    fn from(value: golem_common::model::AgentStatus) -> Self {
        match value {
            golem_common::model::AgentStatus::Running => golem_api_1_x::host::AgentStatus::Running,
            golem_common::model::AgentStatus::Idle => golem_api_1_x::host::AgentStatus::Idle,
            golem_common::model::AgentStatus::Suspended => {
                golem_api_1_x::host::AgentStatus::Suspended
            }
            golem_common::model::AgentStatus::Interrupted => {
                golem_api_1_x::host::AgentStatus::Interrupted
            }
            golem_common::model::AgentStatus::Retrying => {
                golem_api_1_x::host::AgentStatus::Retrying
            }
            golem_common::model::AgentStatus::Failed => golem_api_1_x::host::AgentStatus::Failed,
            golem_common::model::AgentStatus::Exited => golem_api_1_x::host::AgentStatus::Exited,
        }
    }
}

impl TryFrom<golem_api_1_x::host::AgentPropertyFilter> for golem_common::model::AgentFilter {
    type Error = String;

    fn try_from(filter: golem_api_1_x::host::AgentPropertyFilter) -> Result<Self, Self::Error> {
        let converted = match filter {
            golem_api_1_x::host::AgentPropertyFilter::Name(filter) => {
                golem_common::model::AgentFilter::new_name(filter.comparator.into(), filter.value)
            }
            golem_api_1_x::host::AgentPropertyFilter::Version(filter) => {
                golem_common::model::AgentFilter::new_revision(
                    filter.comparator.into(),
                    filter.value.try_into()?,
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::Status(filter) => {
                golem_common::model::AgentFilter::new_status(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::Env(filter) => {
                golem_common::model::AgentFilter::new_env(
                    filter.name,
                    filter.comparator.into(),
                    filter.value,
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::CreatedAt(filter) => {
                golem_common::model::AgentFilter::new_created_at(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::Config(filter) => {
                golem_common::model::AgentFilter::new_config(
                    filter.name,
                    filter.comparator.into(),
                    filter.value,
                )
            }
        };
        Ok(converted)
    }
}

impl TryFrom<golem_api_1_x::host::AgentAllFilter> for golem_common::model::AgentFilter {
    type Error = String;
    fn try_from(filter: golem_api_1_x::host::AgentAllFilter) -> Result<Self, Self::Error> {
        let filters = filter
            .filters
            .into_iter()
            .map(|f| f.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(golem_common::model::AgentFilter::new_and(filters))
    }
}

impl TryFrom<AgentAnyFilter> for golem_common::model::AgentFilter {
    type Error = String;
    fn try_from(filter: AgentAnyFilter) -> Result<Self, Self::Error> {
        let filters = filter
            .filters
            .into_iter()
            .map(|f| f.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(golem_common::model::AgentFilter::new_or(filters))
    }
}

impl From<AgentMetadataForGuests> for golem_api_1_x::host::AgentMetadata {
    fn from(value: AgentMetadataForGuests) -> Self {
        Self {
            agent_id: value.agent_id.into(),
            args: vec![],
            env: value.env,
            config: value.config.into_iter().collect(),
            status: value.status.into(),
            component_revision: value.component_revision.into(),
            retry_count: 0,
            environment_id: value.environment_id.into(),
        }
    }
}

pub struct GetAgentsEntry {
    component_id: ComponentId,
    filter: Option<golem_common::model::AgentFilter>,
    precise: bool,
    count: u64,
    next_cursor: Option<ScanCursor>,
}

impl GetAgentsEntry {
    pub fn new(
        component_id: ComponentId,
        filter: Option<golem_common::model::AgentFilter>,
        precise: bool,
    ) -> Self {
        Self {
            component_id,
            filter,
            precise,
            count: 50,
            next_cursor: Some(ScanCursor::default()),
        }
    }

    fn set_next_cursor(&mut self, cursor: Option<ScanCursor>) {
        self.next_cursor = cursor;
    }
}

#[derive(Clone)]
pub struct GetPromiseResultEntry {
    promise_id: PromiseId,
    promise_service: Arc<dyn PromiseService>,
    handle: Arc<OnceCell<Result<PromiseHandle, String>>>,
}

impl GetPromiseResultEntry {
    pub fn new(promise_id: PromiseId, promise_service: Arc<dyn PromiseService>) -> Self {
        Self {
            promise_id,
            promise_service,
            handle: Arc::new(OnceCell::new()),
        }
    }

    pub async fn get_handle(&self) -> Result<&PromiseHandle, &String> {
        self.handle
            .get_or_init(|| async {
                self.promise_service
                    .poll(self.promise_id.clone())
                    .await
                    .map_err(|err| {
                        format!(
                            "Failed constructing backing promise handle for {}: {err}",
                            self.promise_id
                        )
                    })
            })
            .await
            .as_ref()
    }

    /// Returns true if the underlying promise handle is ready, OR if constructing the
    /// handle failed (so that the pollable resolves immediately and the cached error
    /// is surfaced on the next `get` call).
    pub async fn is_ready(&self) -> bool {
        match self.get_handle().await {
            Ok(handle) => handle.is_ready().await,
            Err(_) => true,
        }
    }
}

// TODO(p3) Blocker 1: re-implement async access via p3 accessor pattern
