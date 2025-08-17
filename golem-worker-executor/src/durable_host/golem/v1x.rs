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

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::get_oplog_entry;
use crate::model::public_oplog::{
    find_component_version_at, get_public_oplog_chunk, search_public_oplog,
};
use crate::preview2::golem_api_1_x;
use crate::preview2::golem_api_1_x::host::{
    ForkResult, GetWorkers, Host, HostGetWorkers, WorkerAnyFilter,
};
use crate::preview2::golem_api_1_x::oplog::{
    Host as OplogHost, HostGetOplog, HostSearchOplog, SearchOplog,
};
use crate::services::oplog::CommitLevel;
use crate::services::{HasOplogService, HasPlugins, HasProjectService, HasWorker};
use crate::workerctx::{InvocationManagement, StatusManagement, WorkerCtx};
use anyhow::anyhow;
use bincode::de::Decoder;
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::Decode;
use golem_common::model::oplog::{DurableFunctionType, OplogEntry};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{ComponentId, ComponentVersion, OwnedWorkerId, ScanCursor, WorkerId};
use golem_common::model::{IdempotencyKey, OplogIndex, PromiseId, RetryConfig};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use std::time::Duration;
use tracing::debug;
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

impl<Ctx: WorkerCtx> HostGetWorkers for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        component_id: golem_api_1_x::host::ComponentId,
        filter: Option<WorkerAnyFilter>,
        precise: bool,
    ) -> anyhow::Result<Resource<GetWorkers>> {
        self.observe_function_call("golem::api::get-workers", "new");
        let entry = GetWorkersEntry::new(component_id.into(), filter.map(|f| f.into()), precise);
        let resource = self.as_wasi_view().table().push(entry)?;
        Ok(resource)
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetWorkers>,
    ) -> anyhow::Result<Option<Vec<golem_api_1_x::host::WorkerMetadata>>> {
        self.observe_function_call("golem::api::get-workers", "get-next");
        let (component_id, filter, count, precise, cursor) = self
            .as_wasi_view()
            .table()
            .get::<GetWorkersEntry>(&self_)
            .map(|e| {
                (
                    e.component_id.clone(),
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
                .get_mut::<GetWorkersEntry>(&self_)
                .map(|e| e.set_next_cursor(new_cursor))?;

            Ok(Some(workers.into_iter().map(|w| w.into()).collect()))
        } else {
            Ok(None)
        }
    }

    async fn drop(&mut self, rep: Resource<GetWorkers>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::get-workers", "drop");
        self.as_wasi_view().table().delete::<GetWorkersEntry>(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn create_promise(&mut self) -> anyhow::Result<golem_api_1_x::host::PromiseId> {
        self.observe_function_call("golem::api", "create_promise");

        let durability = Durability::<PromiseId, SerializableError>::new(
            self,
            "golem::api",
            "create_promise",
            DurableFunctionType::WriteLocal,
        )
        .await?;

        let promise_id = if durability.is_live() {
            let oplog_idx = self.state.current_oplog_index().await.next();
            let promise_id = self
                .public_state
                .promise_service
                .create(&self.owned_worker_id.worker_id, oplog_idx)
                .await;
            durability
                .persist(self, (), Ok::<PromiseId, WorkerExecutorError>(promise_id))
                .await?
        } else {
            durability
                .replay::<PromiseId, WorkerExecutorError>(self)
                .await?
        };

        Ok(promise_id.into())
    }

    async fn await_promise(
        &mut self,
        promise_id: golem_api_1_x::host::PromiseId,
    ) -> anyhow::Result<Vec<u8>> {
        self.observe_function_call("golem::api", "await_promise");
        let promise_id: PromiseId = promise_id.into();
        match self
            .public_state
            .promise_service
            .poll(promise_id.clone())
            .await?
        {
            Some(result) => Ok(result),
            None => {
                debug!("Suspending worker until {} gets completed", promise_id);
                Err(InterruptKind::Suspend.into())
            }
        }
    }

    async fn poll_promise(
        &mut self,
        promise_id: golem_api_1_x::host::PromiseId,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let durability = Durability::<Option<Vec<u8>>, SerializableError>::new(
            self,
            "golem::api",
            "poll_promise",
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let promise_id: PromiseId = promise_id.into();
            let result = self
                .public_state
                .promise_service
                .poll(promise_id.clone())
                .await;
            durability.persist(self, promise_id, result.clone()).await
        } else {
            durability.replay(self).await
        };

        Ok(result?)
    }

    async fn complete_promise(
        &mut self,
        promise_id: golem_api_1_x::host::PromiseId,
        data: Vec<u8>,
    ) -> anyhow::Result<bool> {
        let durability = Durability::<bool, SerializableError>::new(
            self,
            "golem::api",
            "complete_promise",
            DurableFunctionType::WriteLocal,
        )
        .await?;

        let promise_id: PromiseId = promise_id.into();
        let result = if durability.is_live() {
            let result = self
                .public_state
                .promise_service
                .complete(promise_id.clone(), data)
                .await;

            durability.persist(self, promise_id, result).await
        } else {
            durability.replay(self).await
        }?;
        Ok(result)
    }

    async fn delete_promise(
        &mut self,
        promise_id: golem_api_1_x::host::PromiseId,
    ) -> anyhow::Result<()> {
        let durability = Durability::<(), SerializableError>::new(
            self,
            "golem::api",
            "delete_promise",
            DurableFunctionType::WriteLocal,
        )
        .await?;

        let promise_id: PromiseId = promise_id.into();
        if durability.is_live() {
            let result = {
                self.public_state
                    .promise_service
                    .delete(promise_id.clone())
                    .await;
                Ok(())
            };
            durability.persist(self, promise_id, result).await
        } else {
            durability.replay(self).await
        }
    }

    async fn get_oplog_index(&mut self) -> anyhow::Result<golem_api_1_x::oplog::OplogIndex> {
        self.observe_function_call("golem::api", "get_oplog_index");
        if self.state.is_live() {
            self.state.oplog.add(OplogEntry::nop()).await;
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
            self.state
                .replay_state
                .add_skipped_region(jump.clone())
                .await;
            self.state
                .oplog
                .add_and_commit(OplogEntry::jump(jump))
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
                    self.state
                        .replay_state
                        .add_skipped_region(deleted_region.clone())
                        .await;
                    self.state
                        .oplog
                        .add_and_commit(OplogEntry::jump(deleted_region))
                        .await;
                }
            }

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
                self.state
                    .oplog
                    .add(OplogEntry::change_persistence_level(new_persistence_level))
                    .await;
                self.state.oplog.commit(CommitLevel::DurableOnly).await;
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
                    self.state
                        .replay_state
                        .add_skipped_region(deleted_region.clone())
                        .await;
                    self.state
                        .oplog
                        .add_and_commit(OplogEntry::jump(deleted_region))
                        .await;
                }
            }

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
        let durability = Durability::<(u64, u64), SerializableError>::new(
            self,
            "golem api",
            "generate_idempotency_key",
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
        let (hi, lo) = if durability.is_live() {
            let key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);
            let uuid = Uuid::parse_str(&key.value.to_string()).unwrap(); // this is guaranteed to be an uuid
            let result: Result<(u64, u64), anyhow::Error> = Ok(uuid.as_u64_pair());
            durability.persist(self, (), result).await
        } else {
            durability.replay(self).await
        }?;
        let uuid = Uuid::from_u64_pair(hi, lo);
        Ok(uuid.into())
    }

    async fn update_worker(
        &mut self,
        worker_id: golem_api_1_x::host::WorkerId,
        target_version: ComponentVersion,
        mode: golem_api_1_x::host::UpdateMode,
    ) -> anyhow::Result<()> {
        let durability = Durability::<(), SerializableError>::new(
            self,
            "golem::api",
            "update-worker",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let worker_id: WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&self.owned_worker_id.project_id, &worker_id);

        let mode = match mode {
            golem_api_1_x::host::UpdateMode::Automatic => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Automatic
            }
            golem_api_1_x::host::UpdateMode::SnapshotBased => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Manual
            }
        };

        if durability.is_live() {
            let result = self
                .state
                .worker_proxy
                .update(&owned_worker_id, target_version, mode)
                .await;
            durability
                .persist(self, (worker_id, target_version, mode), result)
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(())
    }

    async fn get_self_metadata(&mut self) -> anyhow::Result<golem_api_1_x::host::WorkerMetadata> {
        self.observe_function_call("golem::api", "get_self_metadata");
        let metadata = self.public_state.worker().get_metadata()?;
        Ok(metadata.into())
    }

    async fn get_worker_metadata(
        &mut self,
        worker_id: golem_api_1_x::host::WorkerId,
    ) -> anyhow::Result<Option<golem_api_1_x::host::WorkerMetadata>> {
        self.observe_function_call("golem::api", "get_worker_metadata");
        let worker_id: WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&self.owned_worker_id.project_id, &worker_id);
        let metadata = self.state.worker_service.get(&owned_worker_id).await;

        match metadata {
            Some(metadata) => {
                let last_known_status = self.get_worker_status_record();
                let updated_metadata = golem_common::model::WorkerMetadata {
                    last_known_status,
                    ..metadata
                };
                Ok(Some(updated_metadata.into()))
            }
            None => Ok(None),
        }
    }

    async fn fork_worker(
        &mut self,
        source_worker_id: golem_api_1_x::host::WorkerId,
        target_worker_id: golem_api_1_x::host::WorkerId,
        oplog_idx_cut_off: golem_api_1_x::host::OplogIndex,
    ) -> anyhow::Result<()> {
        let durability = Durability::<(), SerializableError>::new(
            self,
            "golem::api",
            "fork_worker",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let source_worker_id: WorkerId = source_worker_id.into();
        let target_worker_id: WorkerId = target_worker_id.into();

        let oplog_index_cut_off: OplogIndex = OplogIndex::from_u64(oplog_idx_cut_off);

        if durability.is_live() {
            let result = self
                .state
                .worker_proxy
                .fork_worker(&source_worker_id, &target_worker_id, &oplog_index_cut_off)
                .await;
            durability
                .persist(
                    self,
                    (source_worker_id, target_worker_id, oplog_idx_cut_off),
                    result,
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(())
    }

    async fn revert_worker(
        &mut self,
        worker_id: golem_api_1_x::host::WorkerId,
        revert_target: golem_api_1_x::host::RevertWorkerTarget,
    ) -> anyhow::Result<()> {
        let durability = Durability::<(), SerializableError>::new(
            self,
            "golem::api",
            "revert_worker",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if durability.is_live() {
            let worker_id: WorkerId = worker_id.into();
            let revert_target: golem_service_base::model::RevertWorkerTarget = revert_target.into();

            let result = self
                .worker_proxy()
                .revert(&worker_id, revert_target.clone())
                .await;
            durability
                .persist(self, (worker_id, revert_target), result)
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(())
    }

    async fn resolve_component_id(
        &mut self,
        component_slug: String,
    ) -> anyhow::Result<Option<golem_api_1_x::host::ComponentId>> {
        let durability = Durability::<Option<ComponentId>, SerializableError>::new(
            self,
            "golem::api",
            "resolve_component_id",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let result = self
                .state
                .component_service
                .resolve_component(
                    component_slug.clone(),
                    self.state.component_metadata.owner.clone(),
                )
                .await;
            durability.persist(self, component_slug, result).await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.map(golem_api_1_x::host::ComponentId::from))
    }

    async fn resolve_worker_id(
        &mut self,
        component_slug: String,
        worker_name: String,
    ) -> anyhow::Result<Option<golem_api_1_x::host::WorkerId>> {
        let component_id = self.resolve_component_id(component_slug).await?;
        Ok(
            component_id.map(|component_id| golem_api_1_x::host::WorkerId {
                component_id,
                worker_name,
            }),
        )
    }

    async fn resolve_worker_id_strict(
        &mut self,
        component_slug: String,
        worker_name: String,
    ) -> anyhow::Result<Option<golem_api_1_x::host::WorkerId>> {
        let durability = Durability::<Option<WorkerId>, SerializableError>::new(
            self,
            "golem::api",
            "resolve_worker_id_strict",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let worker_id: Result<_, WorkerExecutorError> = async {
                let component_id = self
                    .state
                    .component_service
                    .resolve_component(
                        component_slug.clone(),
                        self.state.component_metadata.owner.clone(),
                    )
                    .await?;
                let worker_id = component_id.map(|component_id| WorkerId {
                    component_id,
                    worker_name: worker_name.clone(),
                });

                if let Some(worker_id) = worker_id.clone() {
                    let owned_id = OwnedWorkerId {
                        project_id: self.state.owned_worker_id.project_id(),
                        worker_id,
                    };

                    let metadata = self.state.worker_service.get(&owned_id).await;

                    if metadata.is_none() {
                        return Ok(None);
                    };
                };
                Ok(worker_id)
            }
            .await;

            durability
                .persist(self, (component_slug, worker_name), worker_id)
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.map(|w| w.into()))
    }

    async fn fork(&mut self, new_name: String) -> anyhow::Result<ForkResult> {
        let durability = Durability::<ForkResult, SerializableError>::new(
            self,
            "golem::api",
            "fork",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if durability.is_live() {
            let target_worker_id = WorkerId {
                component_id: self.owned_worker_id.component_id(),
                worker_name: new_name.clone(),
            };
            let oplog_index_cut_off = self.state.current_oplog_index().await.previous();

            let metadata = self
                .state
                .worker_service
                .get(&self.owned_worker_id)
                .await
                .ok_or_else(|| anyhow::anyhow!("Worker does not exist"))?;
            let fork_result = self
                .state
                .worker_fork
                .fork_and_write_fork_result(
                    &metadata.created_by,
                    &self.owned_worker_id,
                    &target_worker_id,
                    oplog_index_cut_off,
                )
                .await
                .map(|_| ForkResult::Original);

            Ok(durability.persist(self, new_name, fork_result).await?)
        } else {
            durability.replay(self).await
        }
    }
}

impl<Ctx: WorkerCtx> HostGetOplog for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        worker_id: golem_api_1_x::oplog::WorkerId,
        start: golem_api_1_x::oplog::OplogIndex,
    ) -> anyhow::Result<Resource<GetOplogEntry>> {
        self.observe_function_call("golem::api::get-oplog", "new");

        let account_id = self.owned_worker_id.project_id();
        let worker_id: WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let start = OplogIndex::from_u64(start);
        let initial_component_version =
            find_component_version_at(self.state.oplog_service(), &owned_worker_id, start).await?;

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
        let plugins = self.state.plugins();
        let project_service = self.state.project_service();

        let entry = self.as_wasi_view().table().get(&self_)?.clone();

        let chunk = get_public_oplog_chunk(
            component_service,
            oplog_service,
            plugins,
            project_service,
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
                .update(chunk.next_oplog_index, chunk.current_component_version);
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

#[derive(Debug, Clone)]
pub struct GetOplogEntry {
    pub owned_worker_id: OwnedWorkerId,
    pub next_oplog_index: OplogIndex,
    pub current_component_version: ComponentVersion,
    pub page_size: usize,
}

impl GetOplogEntry {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        initial_oplog_index: OplogIndex,
        initial_component_version: ComponentVersion,
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
        current_component_version: ComponentVersion,
    ) {
        self.next_oplog_index = next_oplog_index;
        self.current_component_version = current_component_version;
    }
}

impl<Ctx: WorkerCtx> HostSearchOplog for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        worker_id: golem_api_1_x::oplog::WorkerId,
        text: String,
    ) -> anyhow::Result<Resource<SearchOplog>> {
        self.observe_function_call("golem::api::search-oplog", "new");

        let account_id = self.owned_worker_id.project_id();
        let worker_id: WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let start = OplogIndex::INITIAL;
        let initial_component_version =
            find_component_version_at(self.state.oplog_service(), &owned_worker_id, start).await?;

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
        let plugins = self.state.plugins();
        let project_service = self.state.project_service();

        let entry = self.as_wasi_view().table().get(&self_)?.clone();

        let chunk = search_public_oplog(
            component_service,
            oplog_service,
            plugins,
            project_service,
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
                .update(chunk.next_oplog_index, chunk.current_component_version);
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

#[derive(Debug, Clone)]
pub struct SearchOplogEntry {
    pub owned_worker_id: OwnedWorkerId,
    pub next_oplog_index: OplogIndex,
    pub current_component_version: ComponentVersion,
    pub page_size: usize,
    pub query: String,
}

impl SearchOplogEntry {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        initial_oplog_index: OplogIndex,
        initial_component_version: ComponentVersion,
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
        current_component_version: ComponentVersion,
    ) {
        self.next_oplog_index = next_oplog_index;
        self.current_component_version = current_component_version;
    }
}

impl<Ctx: WorkerCtx> OplogHost for DurableWorkerCtx<Ctx> {}

impl From<golem_api_1_x::host::RevertWorkerTarget>
    for golem_service_base::model::RevertWorkerTarget
{
    fn from(value: golem_api_1_x::host::RevertWorkerTarget) -> Self {
        match value {
            golem_api_1_x::host::RevertWorkerTarget::RevertToOplogIndex(index) => {
                golem_service_base::model::RevertWorkerTarget::RevertToOplogIndex(
                    golem_service_base::model::RevertToOplogIndex {
                        last_oplog_index: OplogIndex::from_u64(index),
                    },
                )
            }
            golem_api_1_x::host::RevertWorkerTarget::RevertLastInvocations(n) => {
                golem_service_base::model::RevertWorkerTarget::RevertLastInvocations(
                    golem_service_base::model::RevertLastInvocations {
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
            worker_id: promise_id.worker_id.into(),
            oplog_idx: promise_id.oplog_idx.into(),
        }
    }
}

impl From<golem_api_1_x::host::PromiseId> for PromiseId {
    fn from(host: golem_api_1_x::host::PromiseId) -> Self {
        Self {
            worker_id: host.worker_id.into(),
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
        }
    }
}

impl From<golem_api_1_x::host::WorkerStatus> for golem_common::model::WorkerStatus {
    fn from(value: golem_api_1_x::host::WorkerStatus) -> Self {
        match value {
            golem_api_1_x::host::WorkerStatus::Running => {
                golem_common::model::WorkerStatus::Running
            }
            golem_api_1_x::host::WorkerStatus::Idle => golem_common::model::WorkerStatus::Idle,
            golem_api_1_x::host::WorkerStatus::Suspended => {
                golem_common::model::WorkerStatus::Suspended
            }
            golem_api_1_x::host::WorkerStatus::Interrupted => {
                golem_common::model::WorkerStatus::Interrupted
            }
            golem_api_1_x::host::WorkerStatus::Retrying => {
                golem_common::model::WorkerStatus::Retrying
            }
            golem_api_1_x::host::WorkerStatus::Failed => golem_common::model::WorkerStatus::Failed,
            golem_api_1_x::host::WorkerStatus::Exited => golem_common::model::WorkerStatus::Exited,
        }
    }
}

impl From<golem_common::model::WorkerStatus> for golem_api_1_x::host::WorkerStatus {
    fn from(value: golem_common::model::WorkerStatus) -> Self {
        match value {
            golem_common::model::WorkerStatus::Running => {
                golem_api_1_x::host::WorkerStatus::Running
            }
            golem_common::model::WorkerStatus::Idle => golem_api_1_x::host::WorkerStatus::Idle,
            golem_common::model::WorkerStatus::Suspended => {
                golem_api_1_x::host::WorkerStatus::Suspended
            }
            golem_common::model::WorkerStatus::Interrupted => {
                golem_api_1_x::host::WorkerStatus::Interrupted
            }
            golem_common::model::WorkerStatus::Retrying => {
                golem_api_1_x::host::WorkerStatus::Retrying
            }
            golem_common::model::WorkerStatus::Failed => golem_api_1_x::host::WorkerStatus::Failed,
            golem_common::model::WorkerStatus::Exited => golem_api_1_x::host::WorkerStatus::Exited,
        }
    }
}

impl From<golem_api_1_x::host::WorkerPropertyFilter> for golem_common::model::WorkerFilter {
    fn from(filter: golem_api_1_x::host::WorkerPropertyFilter) -> Self {
        match filter {
            golem_api_1_x::host::WorkerPropertyFilter::Name(filter) => {
                golem_common::model::WorkerFilter::new_name(filter.comparator.into(), filter.value)
            }
            golem_api_1_x::host::WorkerPropertyFilter::Version(filter) => {
                golem_common::model::WorkerFilter::new_version(
                    filter.comparator.into(),
                    filter.value,
                )
            }
            golem_api_1_x::host::WorkerPropertyFilter::Status(filter) => {
                golem_common::model::WorkerFilter::new_status(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
            golem_api_1_x::host::WorkerPropertyFilter::Env(filter) => {
                golem_common::model::WorkerFilter::new_env(
                    filter.name,
                    filter.comparator.into(),
                    filter.value,
                )
            }
            golem_api_1_x::host::WorkerPropertyFilter::CreatedAt(filter) => {
                golem_common::model::WorkerFilter::new_created_at(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
            golem_api_1_x::host::WorkerPropertyFilter::WasiConfigVars(filter) => {
                golem_common::model::WorkerFilter::new_wasi_config_vars(
                    filter.name,
                    filter.comparator.into(),
                    filter.value,
                )
            }
        }
    }
}

impl From<golem_api_1_x::host::WorkerAllFilter> for golem_common::model::WorkerFilter {
    fn from(filter: golem_api_1_x::host::WorkerAllFilter) -> Self {
        let filters = filter.filters.into_iter().map(|f| f.into()).collect();
        golem_common::model::WorkerFilter::new_and(filters)
    }
}

impl From<WorkerAnyFilter> for golem_common::model::WorkerFilter {
    fn from(filter: WorkerAnyFilter) -> Self {
        let filters = filter.filters.into_iter().map(|f| f.into()).collect();
        golem_common::model::WorkerFilter::new_or(filters)
    }
}

impl From<golem_common::model::WorkerMetadata> for golem_api_1_x::host::WorkerMetadata {
    fn from(value: golem_common::model::WorkerMetadata) -> Self {
        Self {
            worker_id: value.worker_id.into(),
            args: value.args,
            env: value.env,
            wasi_config_vars: value.wasi_config_vars.into_iter().collect(),
            status: value.last_known_status.status.into(),
            component_version: value.last_known_status.component_version,
            retry_count: 0,
        }
    }
}

pub struct GetWorkersEntry {
    component_id: ComponentId,
    filter: Option<golem_common::model::WorkerFilter>,
    precise: bool,
    count: u64,
    next_cursor: Option<ScanCursor>,
}

impl GetWorkersEntry {
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

impl bincode::Encode for ForkResult {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        match self {
            ForkResult::Original => bincode::Encode::encode(&0u8, encoder),
            ForkResult::Forked => bincode::Encode::encode(&1u8, encoder),
        }
    }
}

impl<Context> Decode<Context> for ForkResult {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let value = <u8 as Decode<Context>>::decode(decoder)?;
        match value {
            0 => Ok(ForkResult::Original),
            1 => Ok(ForkResult::Forked),
            _ => Err(DecodeError::Other("Invalid ForkResult")),
        }
    }
}
