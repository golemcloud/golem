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

use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::get_oplog_entry;
use crate::model::public_oplog::{
    find_component_revision_at, get_public_oplog_chunk, search_public_oplog,
};
use crate::preview2::golem_api_1_x::host::{
    AgentAnyFilter, ForkDetails, ForkResult, GetAgents, Host, HostGetAgents, HostGetPromiseResult,
};
use crate::preview2::golem_api_1_x::oplog::{
    Host as OplogHost, HostGetOplog, HostSearchOplog, SearchOplog,
};
use crate::preview2::{golem_api_1_x, Pollable};
use crate::services::oplog::CommitLevel;
use crate::services::promise::{PromiseHandle, PromiseService};
use crate::services::{HasOplogService, HasWorker};
use crate::worker::status::calculate_last_known_status;
use crate::workerctx::{InvocationManagement, StatusManagement, WorkerCtx};
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::agent::AgentId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::oplog::host_functions::{
    GolemApiCompletePromise, GolemApiCreatePromise, GolemApiFork, GolemApiForkWorker,
    GolemApiGenerateIdempotencyKey, GolemApiGetAgentMetadata, GolemApiGetPromiseResult,
    GolemApiGetSelfMetadata, GolemApiResolveComponentId, GolemApiResolveWorkerIdStrict,
    GolemApiRevertWorker, GolemApiUpdateWorker,
};
use golem_common::model::oplog::types::AgentMetadataForGuests;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemApiAgentId, HostRequestGolemApiComponentSlug,
    HostRequestGolemApiComponentSlugAndAgentName, HostRequestGolemApiForkAgent,
    HostRequestGolemApiPromiseId, HostRequestGolemApiRevertAgent, HostRequestGolemApiUpdateAgent,
    HostRequestNoInput, HostResponseGolemApiAgentId, HostResponseGolemApiAgentMetadata,
    HostResponseGolemApiComponentId, HostResponseGolemApiFork, HostResponseGolemApiIdempotencyKey,
    HostResponseGolemApiPromiseCompletion, HostResponseGolemApiPromiseId,
    HostResponseGolemApiPromiseResult, HostResponseGolemApiSelfAgentMetadata,
    HostResponseGolemApiUnit, OplogEntry,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{IdempotencyKey, OplogIndex, PromiseId, RetryConfig};
use golem_common::model::{OwnedWorkerId, ScanCursor, WorkerId};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::debug;
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::{subscribe, IoView};

impl<Ctx: WorkerCtx> HostGetAgents for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        component_id: golem_api_1_x::host::ComponentId,
        filter: Option<AgentAnyFilter>,
        precise: bool,
    ) -> anyhow::Result<Resource<GetAgents>> {
        self.observe_function_call("golem::api::get-workers", "new");
        let entry = GetAgentsEntry::new(component_id.into(), filter.map(|f| f.into()), precise);
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
        let durability =
            Durability::<GolemApiCreatePromise>::new(self, DurableFunctionType::WriteLocal).await?;

        let result = if durability.is_live() {
            let oplog_idx = self.state.current_oplog_index().await.next();
            let promise_id = self
                .public_state
                .promise_service
                .create(&self.owned_worker_id.worker_id, oplog_idx)
                .await;
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseGolemApiPromiseId { promise_id },
                )
                .await?
        } else {
            durability.replay(self).await?
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
        let durability =
            Durability::<GolemApiCompletePromise>::new(self, DurableFunctionType::WriteLocal)
                .await?;

        let promise_id: PromiseId = promise_id.into();
        let result = if durability.is_live() {
            // A promise must be completed on the instance that is owning the agent that originally created here.
            let worker_id = &promise_id.worker_id;

            let is_local_worker = match self.state.shard_service.check_worker(worker_id) {
                Ok(()) => true,
                Err(WorkerExecutorError::InvalidShardId { .. }) => false,
                Err(other) => Err(other)?,
            };

            let promise_completion_result = if is_local_worker {
                self.public_state
                    .promise_service
                    .complete(promise_id.clone(), data, self.created_by())
                    .await?
            } else {
                // talk to the executor that actually owns the promise
                self.state
                    .worker_proxy
                    .complete_promise(promise_id.clone(), data, self.created_by())
                    .await?
            };

            durability
                .persist(
                    self,
                    HostRequestGolemApiPromiseId { promise_id },
                    HostResponseGolemApiPromiseCompletion {
                        completed: promise_completion_result,
                    },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.completed)
    }

    async fn get_oplog_index(&mut self) -> anyhow::Result<golem_api_1_x::oplog::OplogIndex> {
        self.observe_function_call("golem::api", "get_oplog_index");
        if self.state.is_live() {
            self.state.oplog.add(OplogEntry::no_op()).await;
            Ok(self.state.current_oplog_index().await.into())
        } else {
            let (oplog_index, _) = get_oplog_entry!(self.state.replay_state, OplogEntry::NoOp)?;
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
        if jump_target > jump_source {
            Err(anyhow!(
                "Attempted to jump forward in oplog to index {jump_target} from {jump_source}"
            ))
        } else if self
            .state
            .replay_state
            .is_in_skipped_region(jump_target)
            .await
        {
            Err(anyhow!(
                        "Attempted to jump to a deleted region in oplog to index {jump_target} from {jump_source}"
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
            self.state
                .oplog
                .add(OplogEntry::begin_atomic_region())
                .await;
            let begin_index = self.state.current_oplog_index().await;
            self.state.active_atomic_regions.push(begin_index);
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
                    debug!("Worker's atomic operation starting at {} is not committed, ignoring persisted entries",  begin_index);

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

            self.state.active_atomic_regions.push(begin_index);
            Ok(begin_index.into())
        }
    }

    async fn mark_end_operation(
        &mut self,
        begin: golem_api_1_x::oplog::OplogIndex,
    ) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "mark_end_operation");
        if self.state.is_live() {
            self.state
                .oplog
                .add(OplogEntry::end_atomic_region(OplogIndex::from_u64(begin)))
                .await;
        } else {
            let (_, _) = get_oplog_entry!(self.state.replay_state, OplogEntry::EndAtomicRegion)?;
        }

        self.state
            .active_atomic_regions
            .retain(|idx| *idx != OplogIndex::from_u64(begin));

        Ok(())
    }

    async fn get_retry_policy(&mut self) -> anyhow::Result<golem_api_1_x::host::RetryPolicy> {
        self.observe_function_call("golem::api", "get_retry_policy");
        match &self.state.overridden_retry_policy {
            Some(policy) => Ok(policy.into()),
            None => Ok((&self.state.config.retry).into()),
        }
    }

    async fn set_retry_policy(
        &mut self,
        new_retry_policy: golem_api_1_x::host::RetryPolicy,
    ) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "set_retry_policy");
        let new_retry_policy: RetryConfig = new_retry_policy.into();
        self.state.overridden_retry_policy = Some(new_retry_policy.clone());

        if self.state.is_live() {
            self.state
                .oplog
                .add(OplogEntry::change_retry_policy(new_retry_policy))
                .await;
        } else {
            let (_, _) = get_oplog_entry!(self.state.replay_state, OplogEntry::ChangeRetryPolicy)?;
        }
        Ok(())
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
        let durability = Durability::<GolemApiGenerateIdempotencyKey>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let current_idempotency_key = self
            .get_current_idempotency_key()
            .await
            .unwrap_or(IdempotencyKey::fresh());
        let oplog_index = self.state.current_oplog_index().await;

        // NOTE: Even though IdempotencyKey::derived is used, we still need to persist this,
        //       because the derived key depends on the oplog index.
        let result = if durability.is_live() {
            let key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);
            let uuid = Uuid::parse_str(&key.value.to_string()).unwrap(); // this is guaranteed to be an uuid
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseGolemApiIdempotencyKey { uuid },
                )
                .await
        } else {
            durability.replay(self).await
        }?;
        Ok(result.uuid.into())
    }

    async fn update_agent(
        &mut self,
        worker_id: golem_api_1_x::host::AgentId,
        target_version: u64,
        mode: golem_api_1_x::host::UpdateMode,
    ) -> anyhow::Result<()> {
        let durability =
            Durability::<GolemApiUpdateWorker>::new(self, DurableFunctionType::WriteRemote).await?;

        let agent_id: WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&self.owned_worker_id.environment_id, &agent_id);

        let mode = match mode {
            golem_api_1_x::host::UpdateMode::Automatic => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Automatic
            }
            golem_api_1_x::host::UpdateMode::SnapshotBased => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Manual
            }
        };

        let result = if durability.is_live() {
            let result = self
                .state
                .worker_proxy
                .update(
                    &owned_worker_id,
                    ComponentRevision(target_version),
                    mode,
                    self.created_by(),
                )
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(
                    self,
                    HostRequestGolemApiUpdateAgent {
                        agent_id,
                        target_revision: ComponentRevision(target_version),
                        mode,
                    },
                    HostResponseGolemApiUnit { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        result.result.map_err(|err| anyhow!(err))
    }

    async fn get_self_metadata(&mut self) -> anyhow::Result<golem_api_1_x::host::AgentMetadata> {
        let durability =
            Durability::<GolemApiGetSelfMetadata>::new(self, DurableFunctionType::ReadLocal)
                .await?;

        let result = if durability.is_live() {
            let metadata = self
                .public_state
                .worker()
                .get_latest_worker_metadata()
                .await
                .into();

            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseGolemApiSelfAgentMetadata { metadata },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.metadata.into())
    }

    async fn get_agent_metadata(
        &mut self,
        agent_id: golem_api_1_x::host::AgentId,
    ) -> anyhow::Result<Option<golem_api_1_x::host::AgentMetadata>> {
        let durability =
            Durability::<GolemApiGetAgentMetadata>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        let agent_id: WorkerId = agent_id.into();

        let result = if durability.is_live() {
            let owned_worker_id =
                OwnedWorkerId::new(&self.owned_worker_id.environment_id, &agent_id);
            let result = self.state.worker_service.get(&owned_worker_id).await;
            let metadata: Option<AgentMetadataForGuests> = if let Some(result) = result {
                let mut metadata = result.initial_worker_metadata;
                if let Some(last_known_status) = &result.last_known_status {
                    metadata.last_known_status = last_known_status.clone();
                }
                if let Some(status) = calculate_last_known_status(
                    &self.state,
                    &owned_worker_id,
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

            durability
                .persist(
                    self,
                    HostRequestGolemApiAgentId { agent_id },
                    HostResponseGolemApiAgentMetadata { metadata },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.metadata.map(|metadata| metadata.into()))
    }

    async fn fork_agent(
        &mut self,
        source_worker_id: golem_api_1_x::host::AgentId,
        target_worker_id: golem_api_1_x::host::AgentId,
        oplog_idx_cut_off: golem_api_1_x::host::OplogIndex,
    ) -> anyhow::Result<()> {
        let durability =
            Durability::<GolemApiForkWorker>::new(self, DurableFunctionType::WriteRemote).await?;

        let source_worker_id: WorkerId = source_worker_id.into();
        let target_worker_id: WorkerId = target_worker_id.into();

        let oplog_index_cut_off: OplogIndex = OplogIndex::from_u64(oplog_idx_cut_off);

        let result = if durability.is_live() {
            let result = self
                .state
                .worker_proxy
                .fork_worker(
                    &source_worker_id,
                    &target_worker_id,
                    &oplog_index_cut_off,
                    self.created_by(),
                )
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(
                    self,
                    HostRequestGolemApiForkAgent {
                        source_agent_id: source_worker_id,
                        target_agent_id: target_worker_id,
                        oplog_index_cut_off,
                    },
                    HostResponseGolemApiUnit { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        result.result.map_err(|err| anyhow!(err))
    }

    async fn revert_agent(
        &mut self,
        agent_id: golem_api_1_x::host::AgentId,
        revert_target: golem_api_1_x::host::RevertAgentTarget,
    ) -> anyhow::Result<()> {
        let durability =
            Durability::<GolemApiRevertWorker>::new(self, DurableFunctionType::WriteRemote).await?;

        let result = if durability.is_live() {
            let agent_id: WorkerId = agent_id.into();
            let target: golem_common::model::worker::RevertWorkerTarget = revert_target.into();

            let result = self
                .worker_proxy()
                .revert(&agent_id, target.clone(), self.created_by())
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(
                    self,
                    HostRequestGolemApiRevertAgent { agent_id, target },
                    HostResponseGolemApiUnit { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        result.result.map_err(|err| anyhow!(err))
    }

    async fn resolve_component_id(
        &mut self,
        component_slug: String,
    ) -> anyhow::Result<Option<golem_api_1_x::host::ComponentId>> {
        let durability =
            Durability::<GolemApiResolveComponentId>::new(self, DurableFunctionType::WriteRemote)
                .await?;

        let result = if durability.is_live() {
            let result = self
                .state
                .component_service
                .resolve_component(
                    component_slug.clone(),
                    self.state.component_metadata.environment_id,
                    self.state.component_metadata.application_id,
                    self.state.component_metadata.account_id,
                )
                .await
                .map_err(|err| err.to_string());
            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(
                    self,
                    HostRequestGolemApiComponentSlug { component_slug },
                    HostResponseGolemApiComponentId { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

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
        let durability = Durability::<GolemApiResolveWorkerIdStrict>::new(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let result = self
                .resolve_agent_id_strict_internal(component_slug.clone(), agent_name.clone())
                .await
                .map_err(|err| err.to_string());

            durability.try_trigger_retry(self, &result).await?;
            durability
                .persist(
                    self,
                    HostRequestGolemApiComponentSlugAndAgentName {
                        component_slug,
                        agent_name,
                    },
                    HostResponseGolemApiAgentId { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        result
            .result
            .map(|opt| opt.map(golem_api_1_x::host::AgentId::from))
            .map_err(|err| anyhow!(err))
    }

    async fn fork(&mut self) -> anyhow::Result<ForkResult> {
        let durability =
            Durability::<GolemApiFork>::new(self, DurableFunctionType::WriteRemote).await?;

        let result = if durability.is_live() {
            let forked_phantom_id = Uuid::new_v4();

            let new_name = if let Some(agent_id) = self.agent_id() {
                AgentId::new(
                    agent_id.agent_type.clone(),
                    agent_id.parameters.clone(),
                    Some(forked_phantom_id),
                )
                .to_string()
            } else {
                format!("{}-{}", self.worker_id().worker_name, forked_phantom_id)
            };

            let target_agent_id = WorkerId {
                component_id: self.owned_worker_id.component_id(),
                worker_name: new_name.clone(),
            };
            let oplog_index_cut_off = self
                .public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await;

            let created_by = self.created_by();
            let fork_result = self
                .state
                .worker_fork
                .fork_and_write_fork_result(
                    created_by,
                    &self.owned_worker_id,
                    &target_agent_id,
                    oplog_index_cut_off,
                    forked_phantom_id,
                )
                .await
                .map(|_| golem_common::model::ForkResult::Original)
                .map_err(|err| err.to_string());

            durability.try_trigger_retry(self, &fork_result).await?;
            Ok(durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseGolemApiFork {
                        forked_phantom_id,
                        result: fork_result,
                    },
                )
                .await?)
        } else {
            durability.replay(self).await
        }?;

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
        worker_id: golem_api_1_x::oplog::AgentId,
        start: golem_api_1_x::oplog::OplogIndex,
    ) -> anyhow::Result<Resource<GetOplogEntry>> {
        self.observe_function_call("golem::api::get-oplog", "new");

        let account_id = self.owned_worker_id.environment_id();
        let worker_id: WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let start = OplogIndex::from_u64(start);
        let initial_component_version =
            find_component_revision_at(self.state.oplog_service(), &owned_worker_id, start).await?;

        let entry = GetOplogEntry::new(owned_worker_id, start, initial_component_version, 100);
        let resource = self.as_wasi_view().table().push(entry)?;
        Ok(resource)
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetOplogEntry>,
    ) -> anyhow::Result<Option<Vec<golem_api_1_x::oplog::OplogEntry>>> {
        self.observe_function_call("golem::api::get-oplog", "get-next");

        let component_service = self.state.component_service.clone();
        let oplog_service = self.state.oplog_service();

        let entry = self.as_wasi_view().table().get(&self_)?.clone();

        let chunk = get_public_oplog_chunk(
            component_service,
            oplog_service,
            &entry.owned_worker_id,
            entry.current_component_version,
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
    async fn subscribe(
        &mut self,
        resource: Resource<GetPromiseResultEntry>,
    ) -> anyhow::Result<Resource<Pollable>> {
        self.observe_function_call("golem::api::promise-result", "subscribe");
        let handle = self.table().get(&resource)?.clone();

        let resource_rep = resource.rep();
        let dyn_pollable = subscribe(self.table(), resource, None)?;
        self.state
            .promise_backed_pollables
            .write()
            .await
            .insert(dyn_pollable.rep(), handle);
        self.state
            .promise_dyn_pollables
            .write()
            .await
            .entry(resource_rep)
            .or_default()
            .insert(dyn_pollable.rep());

        Ok(dyn_pollable)
    }

    async fn get(
        &mut self,
        resource: Resource<GetPromiseResultEntry>,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let durability =
            Durability::<GolemApiGetPromiseResult>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        let result = if durability.is_live() {
            let self_worker_id = self.worker_id().clone();
            let entry = self.table().get(&resource)?;

            // only the agent that originally created the promise is woken up when it is completed.
            if entry.promise_id.worker_id != self_worker_id {
                return Err(anyhow!(
                    "Tried awaiting a promise not created by the current agent"
                ));
            }

            let result = entry.get_handle().await.get().await;
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseGolemApiPromiseResult { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.result)
    }

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

#[derive(Debug, Clone)]
pub struct GetOplogEntry {
    pub owned_worker_id: OwnedWorkerId,
    pub next_oplog_index: OplogIndex,
    pub current_component_version: ComponentRevision,
    pub page_size: usize,
}

impl GetOplogEntry {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        initial_oplog_index: OplogIndex,
        initial_component_version: ComponentRevision,
        page_size: usize,
    ) -> Self {
        Self {
            owned_worker_id,
            next_oplog_index: initial_oplog_index,
            current_component_version: initial_component_version,
            page_size,
        }
    }

    pub fn update(
        &mut self,
        next_oplog_index: OplogIndex,
        current_component_version: ComponentRevision,
    ) {
        self.next_oplog_index = next_oplog_index;
        self.current_component_version = current_component_version;
    }
}

impl<Ctx: WorkerCtx> HostSearchOplog for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        worker_id: golem_api_1_x::oplog::AgentId,
        text: String,
    ) -> anyhow::Result<Resource<SearchOplog>> {
        self.observe_function_call("golem::api::search-oplog", "new");

        let account_id = self.owned_worker_id.environment_id();
        let worker_id: WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let start = OplogIndex::INITIAL;
        let initial_component_version =
            find_component_revision_at(self.state.oplog_service(), &owned_worker_id, start).await?;

        let entry =
            SearchOplogEntry::new(owned_worker_id, start, initial_component_version, 100, text);
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
                golem_api_1_x::oplog::OplogEntry,
            )>,
        >,
    > {
        self.observe_function_call("golem::api::search-oplog", "get-next");

        let component_service = self.state.component_service.clone();
        let oplog_service = self.state.oplog_service();

        let entry = self.as_wasi_view().table().get(&self_)?.clone();

        let chunk = search_public_oplog(
            component_service,
            oplog_service,
            &entry.owned_worker_id,
            entry.current_component_version,
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
                        let entry: golem_api_1_x::oplog::OplogEntry = entry.into();
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
        worker_name: String,
    ) -> Result<Option<WorkerId>, WorkerExecutorError> {
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

        let worker_id = component_id.map(|component_id| WorkerId {
            component_id,
            worker_name: worker_name.clone(),
        });

        if let Some(worker_id) = worker_id.clone() {
            let owned_id = OwnedWorkerId {
                environment_id: self.state.owned_worker_id.environment_id(),
                worker_id,
            };

            let metadata = self.state.worker_service.get(&owned_id).await;

            if metadata.is_none() {
                return Ok(None);
            };
        };
        Ok(worker_id)
    }
}

#[derive(Debug, Clone)]
pub struct SearchOplogEntry {
    pub owned_worker_id: OwnedWorkerId,
    pub next_oplog_index: OplogIndex,
    pub current_component_version: ComponentRevision,
    pub page_size: usize,
    pub query: String,
}

impl SearchOplogEntry {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        initial_oplog_index: OplogIndex,
        initial_component_version: ComponentRevision,
        page_size: usize,
        query: String,
    ) -> Self {
        Self {
            owned_worker_id,
            next_oplog_index: initial_oplog_index,
            current_component_version: initial_component_version,
            page_size,
            query,
        }
    }

    pub fn update(
        &mut self,
        next_oplog_index: OplogIndex,
        current_component_version: ComponentRevision,
    ) {
        self.next_oplog_index = next_oplog_index;
        self.current_component_version = current_component_version;
    }
}

impl<Ctx: WorkerCtx> OplogHost for DurableWorkerCtx<Ctx> {}

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

impl From<PromiseId> for golem_api_1_x::host::PromiseId {
    fn from(promise_id: PromiseId) -> Self {
        golem_api_1_x::host::PromiseId {
            agent_id: promise_id.worker_id.into(),
            oplog_idx: promise_id.oplog_idx.into(),
        }
    }
}

impl From<golem_api_1_x::host::PromiseId> for PromiseId {
    fn from(host: golem_api_1_x::host::PromiseId) -> Self {
        Self {
            worker_id: host.agent_id.into(),
            oplog_idx: OplogIndex::from_u64(host.oplog_idx),
        }
    }
}

impl From<&RetryConfig> for golem_api_1_x::host::RetryPolicy {
    fn from(value: &RetryConfig) -> Self {
        Self {
            max_attempts: value.max_attempts,
            min_delay: value.min_delay.as_nanos() as u64,
            max_delay: value.max_delay.as_nanos() as u64,
            multiplier: value.multiplier,
            max_jitter_factor: value.max_jitter_factor,
        }
    }
}

impl From<golem_api_1_x::host::RetryPolicy> for RetryConfig {
    fn from(value: golem_api_1_x::host::RetryPolicy) -> Self {
        Self {
            max_attempts: value.max_attempts,
            min_delay: Duration::from_nanos(value.min_delay),
            max_delay: Duration::from_nanos(value.max_delay),
            multiplier: value.multiplier,
            max_jitter_factor: None,
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

impl From<golem_api_1_x::host::AgentStatus> for golem_common::model::WorkerStatus {
    fn from(value: golem_api_1_x::host::AgentStatus) -> Self {
        match value {
            golem_api_1_x::host::AgentStatus::Running => golem_common::model::WorkerStatus::Running,
            golem_api_1_x::host::AgentStatus::Idle => golem_common::model::WorkerStatus::Idle,
            golem_api_1_x::host::AgentStatus::Suspended => {
                golem_common::model::WorkerStatus::Suspended
            }
            golem_api_1_x::host::AgentStatus::Interrupted => {
                golem_common::model::WorkerStatus::Interrupted
            }
            golem_api_1_x::host::AgentStatus::Retrying => {
                golem_common::model::WorkerStatus::Retrying
            }
            golem_api_1_x::host::AgentStatus::Failed => golem_common::model::WorkerStatus::Failed,
            golem_api_1_x::host::AgentStatus::Exited => golem_common::model::WorkerStatus::Exited,
        }
    }
}

impl From<golem_common::model::WorkerStatus> for golem_api_1_x::host::AgentStatus {
    fn from(value: golem_common::model::WorkerStatus) -> Self {
        match value {
            golem_common::model::WorkerStatus::Running => golem_api_1_x::host::AgentStatus::Running,
            golem_common::model::WorkerStatus::Idle => golem_api_1_x::host::AgentStatus::Idle,
            golem_common::model::WorkerStatus::Suspended => {
                golem_api_1_x::host::AgentStatus::Suspended
            }
            golem_common::model::WorkerStatus::Interrupted => {
                golem_api_1_x::host::AgentStatus::Interrupted
            }
            golem_common::model::WorkerStatus::Retrying => {
                golem_api_1_x::host::AgentStatus::Retrying
            }
            golem_common::model::WorkerStatus::Failed => golem_api_1_x::host::AgentStatus::Failed,
            golem_common::model::WorkerStatus::Exited => golem_api_1_x::host::AgentStatus::Exited,
        }
    }
}

impl From<golem_api_1_x::host::AgentPropertyFilter> for golem_common::model::WorkerFilter {
    fn from(filter: golem_api_1_x::host::AgentPropertyFilter) -> Self {
        match filter {
            golem_api_1_x::host::AgentPropertyFilter::Name(filter) => {
                golem_common::model::WorkerFilter::new_name(filter.comparator.into(), filter.value)
            }
            golem_api_1_x::host::AgentPropertyFilter::Version(filter) => {
                golem_common::model::WorkerFilter::new_version(
                    filter.comparator.into(),
                    ComponentRevision(filter.value),
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::Status(filter) => {
                golem_common::model::WorkerFilter::new_status(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::Env(filter) => {
                golem_common::model::WorkerFilter::new_env(
                    filter.name,
                    filter.comparator.into(),
                    filter.value,
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::CreatedAt(filter) => {
                golem_common::model::WorkerFilter::new_created_at(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::WasiConfigVars(filter) => {
                golem_common::model::WorkerFilter::new_wasi_config_vars(
                    filter.name,
                    filter.comparator.into(),
                    filter.value,
                )
            }
        }
    }
}

impl From<golem_api_1_x::host::AgentAllFilter> for golem_common::model::WorkerFilter {
    fn from(filter: golem_api_1_x::host::AgentAllFilter) -> Self {
        let filters = filter.filters.into_iter().map(|f| f.into()).collect();
        golem_common::model::WorkerFilter::new_and(filters)
    }
}

impl From<AgentAnyFilter> for golem_common::model::WorkerFilter {
    fn from(filter: AgentAnyFilter) -> Self {
        let filters = filter.filters.into_iter().map(|f| f.into()).collect();
        golem_common::model::WorkerFilter::new_or(filters)
    }
}

impl From<AgentMetadataForGuests> for golem_api_1_x::host::AgentMetadata {
    fn from(value: AgentMetadataForGuests) -> Self {
        Self {
            agent_id: value.agent_id.into(),
            args: vec![],
            env: value.env,
            config_vars: value.config_vars.into_iter().collect(),
            status: value.status.into(),
            component_revision: value.component_revision.0,
            retry_count: 0,
        }
    }
}

pub struct GetAgentsEntry {
    component_id: ComponentId,
    filter: Option<golem_common::model::WorkerFilter>,
    precise: bool,
    count: u64,
    next_cursor: Option<ScanCursor>,
}

impl GetAgentsEntry {
    pub fn new(
        component_id: ComponentId,
        filter: Option<golem_common::model::WorkerFilter>,
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
    handle: Arc<OnceCell<PromiseHandle>>,
}

impl GetPromiseResultEntry {
    pub fn new(promise_id: PromiseId, promise_service: Arc<dyn PromiseService>) -> Self {
        Self {
            promise_id,
            promise_service,
            handle: Arc::new(OnceCell::new()),
        }
    }

    pub async fn get_handle(&self) -> &PromiseHandle {
        self.handle
            .get_or_init(|| async {
                self.promise_service
                    .poll(self.promise_id.clone())
                    .await
                    .expect("Failed constructing backing promise handle")
            })
            .await
    }
}

#[async_trait]
impl wasmtime_wasi::p2::Pollable for GetPromiseResultEntry {
    async fn ready(&mut self) {
        self.get_handle().await.await_ready().await
    }
}
