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

pub mod v11;

use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::RetryConfig;
use std::time::Duration;
use tracing::debug;
use uuid::Uuid;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::wasm_rpc::UrnExtensions;
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::get_oplog_entry;
use crate::model::InterruptKind;
use crate::preview2::golem;
use crate::preview2::golem::api0_2_0::host::{
    ComponentVersion, HostGetWorkers, PersistenceLevel, RetryPolicy, UpdateMode, WorkerMetadata,
};
use crate::services::oplog::CommitLevel;
use crate::services::HasWorker;
use crate::workerctx::{InvocationManagement, StatusManagement, WorkerCtx};
use golem_common::model::oplog::{DurableFunctionType, OplogEntry, OplogIndex};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{
    ComponentId, IdempotencyKey, OwnedWorkerId, PromiseId, ScanCursor, WorkerId,
};

#[async_trait]
impl<Ctx: WorkerCtx> HostGetWorkers for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        component_id: golem::api0_2_0::host::ComponentId,
        filter: Option<golem::api0_2_0::host::WorkerAnyFilter>,
        precise: bool,
    ) -> anyhow::Result<Resource<GetWorkersEntry>> {
        self.observe_function_call("golem::api::get-workers", "new");
        let entry = GetWorkersEntry::new(component_id.into(), filter.map(|f| f.into()), precise);
        let resource = self.as_wasi_view().table().push(entry)?;
        Ok(resource)
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetWorkersEntry>,
    ) -> anyhow::Result<Option<Vec<WorkerMetadata>>> {
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

            let _ = self
                .as_wasi_view()
                .table()
                .get_mut::<GetWorkersEntry>(&self_)
                .map(|e| e.set_next_cursor(new_cursor))?;

            Ok(Some(workers.into_iter().map(|w| w.into()).collect()))
        } else {
            Ok(None)
        }
    }

    async fn drop(&mut self, rep: Resource<GetWorkersEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::get-workers", "drop");
        self.as_wasi_view().table().delete::<GetWorkersEntry>(rep)?;
        Ok(())
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

#[async_trait]
impl<Ctx: WorkerCtx> golem::api0_2_0::host::Host for DurableWorkerCtx<Ctx> {
    async fn create_promise(&mut self) -> Result<golem::api0_2_0::host::PromiseId, anyhow::Error> {
        self.observe_function_call("golem::api", "create_promise");
        let oplog_idx = self.get_oplog_index().await?;

        Ok(self
            .public_state
            .promise_service
            .create(
                &self.owned_worker_id.worker_id,
                OplogIndex::from_u64(oplog_idx),
            )
            .await
            .into())
    }

    async fn await_promise(
        &mut self,
        promise_id: golem::api0_2_0::host::PromiseId,
    ) -> Result<Vec<u8>, anyhow::Error> {
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

    async fn complete_promise(
        &mut self,
        promise_id: golem::api0_2_0::host::PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, anyhow::Error> {
        let durability = Durability::<bool, SerializableError>::new(
            self,
            "", // TODO: fix in 2.0
            "golem_complete_promise",
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
        promise_id: golem::api0_2_0::host::PromiseId,
    ) -> Result<(), anyhow::Error> {
        let durability = Durability::<(), SerializableError>::new(
            self,
            "", // TODO: fix in 2.0
            "golem_delete_promise",
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

    async fn get_self_uri(
        &mut self,
        function_name: String,
    ) -> Result<golem::rpc::types::Uri, anyhow::Error> {
        self.observe_function_call("golem::api", "get_self_uri");
        let uri = golem_wasm_rpc::golem::rpc::types::Uri::golem_urn(
            &self.owned_worker_id.worker_id,
            Some(&function_name),
        );
        Ok(golem::rpc::types::Uri { value: uri.value })
    }

    async fn get_oplog_index(&mut self) -> anyhow::Result<golem::api0_2_0::host::OplogIndex> {
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
        oplog_idx: golem::api0_2_0::host::OplogIndex,
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
            .is_in_deleted_region(jump_target)
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
                .add_deleted_region(jump.clone())
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

    async fn mark_begin_operation(&mut self) -> anyhow::Result<golem::api0_2_0::host::OplogIndex> {
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
                    self.state.replay_state.switch_to_live();

                    // But this is not enough, because if the retried transactional block succeeds,
                    // and later we replay it, we need to skip the first attempt and only replay the second.
                    // Se we add a Jump entry to the oplog that registers a deleted region.
                    let deleted_region = OplogRegion {
                        start: begin_index.next(), // need to keep the BeginAtomicRegion entry
                        end: self.state.replay_state.replay_target().next(), // skipping the Jump entry too
                    };
                    self.state
                        .replay_state
                        .add_deleted_region(deleted_region.clone())
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
        begin: golem::api0_2_0::host::OplogIndex,
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

    async fn get_retry_policy(&mut self) -> anyhow::Result<RetryPolicy> {
        self.observe_function_call("golem::api", "get_retry_policy");
        match &self.state.overridden_retry_policy {
            Some(policy) => Ok(policy.into()),
            None => Ok((&self.state.config.retry).into()),
        }
    }

    async fn set_retry_policy(&mut self, new_retry_policy: RetryPolicy) -> anyhow::Result<()> {
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

    async fn get_oplog_persistence_level(&mut self) -> anyhow::Result<PersistenceLevel> {
        self.observe_function_call("golem::api", "get_oplog_persistence_level");
        Ok(self.state.persistence_level.into())
    }

    async fn set_oplog_persistence_level(
        &mut self,
        new_persistence_level: PersistenceLevel,
    ) -> anyhow::Result<()> {
        self.observe_function_call("golem::api", "set_oplog_persistence_level");
        // commit all pending entries and change persistence level
        if self.state.is_live() {
            self.state.oplog.commit(CommitLevel::DurableOnly).await;
        }
        self.state.persistence_level = new_persistence_level.into();
        debug!(
            "Worker's oplog persistence level is set to {:?}",
            self.state.persistence_level
        );
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

    async fn generate_idempotency_key(&mut self) -> anyhow::Result<golem::api0_2_0::host::Uuid> {
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

        // TODO: Fix in 2.0 - Now that IdempotencyKey::derived is used, we no longer need to persist this, but we do to avoid breaking existing oplogs
        let (hi, lo) = if durability.is_live() {
            let key = IdempotencyKey::derived(&current_idempotency_key, oplog_index);
            let uuid = Uuid::parse_str(&key.value.to_string()).unwrap(); // this is guaranteed to be a uuid
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
        worker_id: golem::api0_2_0::host::WorkerId,
        target_version: ComponentVersion,
        mode: UpdateMode,
    ) -> anyhow::Result<()> {
        let durability = Durability::<(), SerializableError>::new(
            self,
            "golem::api",
            "update-worker",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let worker_id: WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&self.owned_worker_id.account_id, &worker_id);

        let mode = match mode {
            UpdateMode::Automatic => golem_api_grpc::proto::golem::worker::UpdateMode::Automatic,
            UpdateMode::SnapshotBased => golem_api_grpc::proto::golem::worker::UpdateMode::Manual,
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

    async fn get_self_metadata(&mut self) -> anyhow::Result<WorkerMetadata> {
        self.observe_function_call("golem::api", "get_self_metadata");
        let metadata = self.public_state.worker().get_metadata().await?;
        Ok(metadata.into())
    }

    async fn get_worker_metadata(
        &mut self,
        worker_id: golem::api0_2_0::host::WorkerId,
    ) -> anyhow::Result<Option<WorkerMetadata>> {
        self.observe_function_call("golem::api", "get_worker_metadata");
        let worker_id: WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&self.owned_worker_id.account_id, &worker_id);
        let metadata = self.state.worker_service.get(&owned_worker_id).await;

        match metadata {
            Some(metadata) => {
                let last_known_status = Ctx::compute_latest_worker_status(
                    &self.state,
                    &owned_worker_id,
                    &Some(metadata.clone()),
                )
                .await?;
                let updated_metadata = golem_common::model::WorkerMetadata {
                    last_known_status,
                    ..metadata
                };
                Ok(Some(updated_metadata.into()))
            }
            None => Ok(None),
        }
    }
}

impl From<WorkerId> for golem::api0_2_0::host::WorkerId {
    fn from(worker_id: WorkerId) -> Self {
        golem::api0_2_0::host::WorkerId {
            component_id: worker_id.component_id.into(),
            worker_name: worker_id.worker_name,
        }
    }
}

impl From<golem::api0_2_0::host::WorkerId> for WorkerId {
    fn from(host: golem::api0_2_0::host::WorkerId) -> Self {
        Self {
            component_id: host.component_id.into(),
            worker_name: host.worker_name,
        }
    }
}

impl From<golem::api0_2_0::host::ComponentId> for ComponentId {
    fn from(host: golem::api0_2_0::host::ComponentId) -> Self {
        let high_bits = host.uuid.high_bits;
        let low_bits = host.uuid.low_bits;

        Self(Uuid::from_u64_pair(high_bits, low_bits))
    }
}

impl From<ComponentId> for golem::api0_2_0::host::ComponentId {
    fn from(component_id: ComponentId) -> Self {
        let (high_bits, low_bits) = component_id.0.as_u64_pair();

        golem::api0_2_0::host::ComponentId {
            uuid: golem::api0_2_0::host::Uuid {
                high_bits,
                low_bits,
            },
        }
    }
}

impl From<PromiseId> for golem::api0_2_0::host::PromiseId {
    fn from(promise_id: PromiseId) -> Self {
        golem::api0_2_0::host::PromiseId {
            worker_id: promise_id.worker_id.into(),
            oplog_idx: promise_id.oplog_idx.into(),
        }
    }
}

impl From<golem::api0_2_0::host::PromiseId> for PromiseId {
    fn from(host: golem::api0_2_0::host::PromiseId) -> Self {
        Self {
            worker_id: host.worker_id.into(),
            oplog_idx: OplogIndex::from_u64(host.oplog_idx),
        }
    }
}

impl From<&RetryConfig> for RetryPolicy {
    fn from(value: &RetryConfig) -> Self {
        Self {
            max_attempts: value.max_attempts,
            min_delay: value.min_delay.as_nanos() as u64,
            max_delay: value.max_delay.as_nanos() as u64,
            multiplier: value.multiplier,
        }
    }
}

impl From<RetryPolicy> for RetryConfig {
    fn from(value: RetryPolicy) -> Self {
        Self {
            max_attempts: value.max_attempts,
            min_delay: Duration::from_nanos(value.min_delay),
            max_delay: Duration::from_nanos(value.max_delay),
            multiplier: value.multiplier,
            max_jitter_factor: None,
        }
    }
}

impl From<Uuid> for golem::api0_2_0::host::Uuid {
    fn from(uuid: Uuid) -> Self {
        let (high_bits, low_bits) = uuid.as_u64_pair();
        golem::api0_2_0::host::Uuid {
            high_bits,
            low_bits,
        }
    }
}

impl From<golem::api0_2_0::host::FilterComparator> for golem_common::model::FilterComparator {
    fn from(value: golem::api0_2_0::host::FilterComparator) -> Self {
        match value {
            golem::api0_2_0::host::FilterComparator::Equal => {
                golem_common::model::FilterComparator::Equal
            }
            golem::api0_2_0::host::FilterComparator::NotEqual => {
                golem_common::model::FilterComparator::NotEqual
            }
            golem::api0_2_0::host::FilterComparator::Less => {
                golem_common::model::FilterComparator::Less
            }
            golem::api0_2_0::host::FilterComparator::LessEqual => {
                golem_common::model::FilterComparator::LessEqual
            }
            golem::api0_2_0::host::FilterComparator::Greater => {
                golem_common::model::FilterComparator::Greater
            }
            golem::api0_2_0::host::FilterComparator::GreaterEqual => {
                golem_common::model::FilterComparator::GreaterEqual
            }
        }
    }
}

impl From<golem::api0_2_0::host::StringFilterComparator>
    for golem_common::model::StringFilterComparator
{
    fn from(value: golem::api0_2_0::host::StringFilterComparator) -> Self {
        match value {
            golem::api0_2_0::host::StringFilterComparator::Equal => {
                golem_common::model::StringFilterComparator::Equal
            }
            golem::api0_2_0::host::StringFilterComparator::NotEqual => {
                golem_common::model::StringFilterComparator::NotEqual
            }
            golem::api0_2_0::host::StringFilterComparator::Like => {
                golem_common::model::StringFilterComparator::Like
            }
            golem::api0_2_0::host::StringFilterComparator::NotLike => {
                golem_common::model::StringFilterComparator::NotLike
            }
        }
    }
}

impl From<golem::api0_2_0::host::WorkerStatus> for golem_common::model::WorkerStatus {
    fn from(value: golem::api0_2_0::host::WorkerStatus) -> Self {
        match value {
            golem::api0_2_0::host::WorkerStatus::Running => {
                golem_common::model::WorkerStatus::Running
            }
            golem::api0_2_0::host::WorkerStatus::Idle => golem_common::model::WorkerStatus::Idle,
            golem::api0_2_0::host::WorkerStatus::Suspended => {
                golem_common::model::WorkerStatus::Suspended
            }
            golem::api0_2_0::host::WorkerStatus::Interrupted => {
                golem_common::model::WorkerStatus::Interrupted
            }
            golem::api0_2_0::host::WorkerStatus::Retrying => {
                golem_common::model::WorkerStatus::Retrying
            }
            golem::api0_2_0::host::WorkerStatus::Failed => {
                golem_common::model::WorkerStatus::Failed
            }
            golem::api0_2_0::host::WorkerStatus::Exited => {
                golem_common::model::WorkerStatus::Exited
            }
        }
    }
}

impl From<golem_common::model::WorkerStatus> for golem::api0_2_0::host::WorkerStatus {
    fn from(value: golem_common::model::WorkerStatus) -> Self {
        match value {
            golem_common::model::WorkerStatus::Running => {
                golem::api0_2_0::host::WorkerStatus::Running
            }
            golem_common::model::WorkerStatus::Idle => golem::api0_2_0::host::WorkerStatus::Idle,
            golem_common::model::WorkerStatus::Suspended => {
                golem::api0_2_0::host::WorkerStatus::Suspended
            }
            golem_common::model::WorkerStatus::Interrupted => {
                golem::api0_2_0::host::WorkerStatus::Interrupted
            }
            golem_common::model::WorkerStatus::Retrying => {
                golem::api0_2_0::host::WorkerStatus::Retrying
            }
            golem_common::model::WorkerStatus::Failed => {
                golem::api0_2_0::host::WorkerStatus::Failed
            }
            golem_common::model::WorkerStatus::Exited => {
                golem::api0_2_0::host::WorkerStatus::Exited
            }
        }
    }
}

impl From<golem::api0_2_0::host::WorkerPropertyFilter> for golem_common::model::WorkerFilter {
    fn from(filter: golem::api0_2_0::host::WorkerPropertyFilter) -> Self {
        match filter {
            golem::api0_2_0::host::WorkerPropertyFilter::Name(filter) => {
                golem_common::model::WorkerFilter::new_name(filter.comparator.into(), filter.value)
            }
            golem::api0_2_0::host::WorkerPropertyFilter::Version(filter) => {
                golem_common::model::WorkerFilter::new_version(
                    filter.comparator.into(),
                    filter.value,
                )
            }
            golem::api0_2_0::host::WorkerPropertyFilter::Status(filter) => {
                golem_common::model::WorkerFilter::new_status(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
            golem::api0_2_0::host::WorkerPropertyFilter::Env(filter) => {
                golem_common::model::WorkerFilter::new_env(
                    filter.name,
                    filter.comparator.into(),
                    filter.value,
                )
            }
            golem::api0_2_0::host::WorkerPropertyFilter::CreatedAt(filter) => {
                golem_common::model::WorkerFilter::new_created_at(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
        }
    }
}

impl From<golem::api0_2_0::host::WorkerAllFilter> for golem_common::model::WorkerFilter {
    fn from(filter: golem::api0_2_0::host::WorkerAllFilter) -> Self {
        let filters = filter.filters.into_iter().map(|f| f.into()).collect();
        golem_common::model::WorkerFilter::new_and(filters)
    }
}

impl From<golem::api0_2_0::host::WorkerAnyFilter> for golem_common::model::WorkerFilter {
    fn from(filter: golem::api0_2_0::host::WorkerAnyFilter) -> Self {
        let filters = filter.filters.into_iter().map(|f| f.into()).collect();
        golem_common::model::WorkerFilter::new_or(filters)
    }
}

impl From<golem_common::model::WorkerMetadata> for WorkerMetadata {
    fn from(value: golem_common::model::WorkerMetadata) -> Self {
        Self {
            worker_id: value.worker_id.into(),
            args: value.args,
            env: value.env,
            status: value.last_known_status.status.into(),
            component_version: value.last_known_status.component_version,
            retry_count: 0,
        }
    }
}
