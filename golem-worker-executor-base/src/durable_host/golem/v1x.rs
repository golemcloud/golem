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

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::model::public_oplog::{
    find_component_version_at, get_public_oplog_chunk, search_public_oplog,
};
use crate::preview2::golem_api_0_2_x;
use crate::preview2::golem_api_0_2_x::host::GetWorkers;
use crate::preview2::golem_api_1_x;
use crate::preview2::golem_api_1_x::host::{
    ComponentId, ComponentVersion, FilterComparator, Host, HostGetWorkers, OplogIndex,
    PersistenceLevel, PromiseId, RetryPolicy, StringFilterComparator, UpdateMode, Uuid,
    WorkerAllFilter, WorkerAnyFilter, WorkerCreatedAtFilter, WorkerEnvFilter, WorkerId,
    WorkerMetadata, WorkerNameFilter, WorkerPropertyFilter, WorkerStatus, WorkerStatusFilter,
    WorkerVersionFilter,
};
use crate::preview2::golem_api_1_x::oplog::{
    Host as OplogHost, HostGetOplog, HostSearchOplog, OplogEntry, SearchOplog,
};
use crate::services::{HasOplogService, HasPlugins};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::OwnedWorkerId;
use golem_common::model::RetryConfig;
use std::time::Duration;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

#[async_trait]
impl<Ctx: WorkerCtx> HostGetWorkers for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        component_id: ComponentId,
        filter: Option<WorkerAnyFilter>,
        precise: bool,
    ) -> anyhow::Result<Resource<GetWorkers>> {
        golem_api_0_2_x::host::HostGetWorkers::new(
            self,
            component_id.into(),
            filter.map(|x| x.into()),
            precise,
        )
        .await
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetWorkers>,
    ) -> anyhow::Result<Option<Vec<WorkerMetadata>>> {
        golem_api_0_2_x::host::HostGetWorkers::get_next(self, self_)
            .await
            .map(|x| x.map(|x| x.into_iter().map(|x| x.into()).collect()))
    }

    async fn drop(&mut self, rep: Resource<GetWorkers>) -> anyhow::Result<()> {
        golem_api_0_2_x::host::HostGetWorkers::drop(self, rep).await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn create_promise(&mut self) -> anyhow::Result<PromiseId> {
        golem_api_0_2_x::host::Host::create_promise(self)
            .await
            .map(|x| x.into())
    }

    async fn await_promise(&mut self, promise_id: PromiseId) -> anyhow::Result<Vec<u8>> {
        golem_api_0_2_x::host::Host::await_promise(self, promise_id.into()).await
    }

    async fn complete_promise(
        &mut self,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> anyhow::Result<bool> {
        golem_api_0_2_x::host::Host::complete_promise(self, promise_id.into(), data).await
    }

    async fn delete_promise(&mut self, promise_id: PromiseId) -> anyhow::Result<()> {
        golem_api_0_2_x::host::Host::delete_promise(self, promise_id.into()).await
    }

    async fn get_oplog_index(&mut self) -> anyhow::Result<OplogIndex> {
        golem_api_0_2_x::host::Host::get_oplog_index(self).await
    }

    async fn set_oplog_index(&mut self, oplog_idx: OplogIndex) -> anyhow::Result<()> {
        golem_api_0_2_x::host::Host::set_oplog_index(self, oplog_idx).await
    }

    async fn oplog_commit(&mut self, replicas: u8) -> anyhow::Result<()> {
        golem_api_0_2_x::host::Host::oplog_commit(self, replicas).await
    }

    async fn mark_begin_operation(&mut self) -> anyhow::Result<OplogIndex> {
        golem_api_0_2_x::host::Host::mark_begin_operation(self).await
    }

    async fn mark_end_operation(&mut self, begin: OplogIndex) -> anyhow::Result<()> {
        golem_api_0_2_x::host::Host::mark_end_operation(self, begin).await
    }

    async fn get_retry_policy(&mut self) -> anyhow::Result<RetryPolicy> {
        golem_api_0_2_x::host::Host::get_retry_policy(self)
            .await
            .map(|x| x.into())
    }

    async fn set_retry_policy(&mut self, new_retry_policy: RetryPolicy) -> anyhow::Result<()> {
        golem_api_0_2_x::host::Host::set_retry_policy(self, new_retry_policy.into()).await
    }

    async fn get_oplog_persistence_level(&mut self) -> anyhow::Result<PersistenceLevel> {
        golem_api_0_2_x::host::Host::get_oplog_persistence_level(self)
            .await
            .map(|x| x.into())
    }

    async fn set_oplog_persistence_level(
        &mut self,
        new_persistence_level: PersistenceLevel,
    ) -> anyhow::Result<()> {
        golem_api_0_2_x::host::Host::set_oplog_persistence_level(self, new_persistence_level.into())
            .await
    }

    async fn get_idempotence_mode(&mut self) -> anyhow::Result<bool> {
        golem_api_0_2_x::host::Host::get_idempotence_mode(self).await
    }

    async fn set_idempotence_mode(&mut self, idempotent: bool) -> anyhow::Result<()> {
        golem_api_0_2_x::host::Host::set_idempotence_mode(self, idempotent).await
    }

    async fn generate_idempotency_key(&mut self) -> anyhow::Result<Uuid> {
        golem_api_0_2_x::host::Host::generate_idempotency_key(self)
            .await
            .map(|x| x.into())
    }

    async fn update_worker(
        &mut self,
        worker_id: WorkerId,
        target_version: ComponentVersion,
        mode: UpdateMode,
    ) -> anyhow::Result<()> {
        golem_api_0_2_x::host::Host::update_worker(
            self,
            worker_id.into(),
            target_version,
            mode.into(),
        )
        .await
    }

    async fn get_self_metadata(&mut self) -> anyhow::Result<WorkerMetadata> {
        golem_api_0_2_x::host::Host::get_self_metadata(self)
            .await
            .map(|x| x.into())
    }

    async fn get_worker_metadata(
        &mut self,
        worker_id: WorkerId,
    ) -> anyhow::Result<Option<WorkerMetadata>> {
        golem_api_0_2_x::host::Host::get_worker_metadata(self, worker_id.into())
            .await
            .map(|x| x.map(|x| x.into()))
    }

    async fn fork_worker(
        &mut self,
        source_worker_id: WorkerId,
        target_worker_id: WorkerId,
        oplog_idx_cut_off: OplogIndex,
    ) -> anyhow::Result<()> {
        let durability = Durability::<(), SerializableError>::new(
            self,
            "golem::api",
            "fork_worker",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let source_worker_id: golem_common::model::WorkerId = source_worker_id.into();

        let target_worker_id: golem_common::model::WorkerId = target_worker_id.into();

        let oplog_index_cut_off: golem_common::model::oplog::OplogIndex =
            golem_common::model::oplog::OplogIndex::from_u64(oplog_idx_cut_off);

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

    async fn schedule_invocation(
        &mut self,
        _: crate::preview2::wasi::clocks::wall_clock::Datetime,
        _: golem_api_1_x::host::WorkerId,
        _: String,
        _: Vec<crate::preview2::golem::rpc::types::WitValue>
    ) -> anyhow::Result<()> {
        todo!()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostGetOplog for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        worker_id: golem_api_1_x::oplog::WorkerId,
        start: golem_api_1_x::oplog::OplogIndex,
    ) -> anyhow::Result<Resource<GetOplogEntry>> {
        self.observe_function_call("golem::api::get-oplog", "new");

        let account_id = self.owned_worker_id.account_id();
        let worker_id: golem_common::model::WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let start = golem_common::model::oplog::OplogIndex::from_u64(start);
        let initial_component_version =
            find_component_version_at(self.state.oplog_service(), &owned_worker_id, start).await?;

        let entry = GetOplogEntry::new(owned_worker_id, start, initial_component_version, 100);
        let resource = self.as_wasi_view().table().push(entry)?;
        Ok(resource)
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetOplogEntry>,
    ) -> anyhow::Result<Option<Vec<OplogEntry>>> {
        self.observe_function_call("golem::api::get-oplog", "get-next");

        let component_service = self.state.component_service.clone();
        let oplog_service = self.state.oplog_service();
        let plugins = self.state.plugins();

        let entry = self.as_wasi_view().table().get(&self_)?.clone();

        let chunk = get_public_oplog_chunk(
            component_service,
            oplog_service,
            plugins,
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
    pub next_oplog_index: golem_common::model::oplog::OplogIndex,
    pub current_component_version: ComponentVersion,
    pub page_size: usize,
}

impl crate::durable_host::golem::v1x::GetOplogEntry {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        initial_oplog_index: golem_common::model::oplog::OplogIndex,
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
        next_oplog_index: golem_common::model::oplog::OplogIndex,
        current_component_version: ComponentVersion,
    ) {
        self.next_oplog_index = next_oplog_index;
        self.current_component_version = current_component_version;
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostSearchOplog for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        worker_id: golem_api_1_x::oplog::WorkerId,
        text: String,
    ) -> anyhow::Result<Resource<SearchOplog>> {
        self.observe_function_call("golem::api::search-oplog", "new");

        let account_id = self.owned_worker_id.account_id();
        let worker_id: golem_common::model::WorkerId = worker_id.into();
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        let start = golem_common::model::oplog::OplogIndex::INITIAL;
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
    ) -> anyhow::Result<Option<Vec<(golem_api_1_x::oplog::OplogIndex, OplogEntry)>>> {
        self.observe_function_call("golem::api::search-oplog", "get-next");

        let component_service = self.state.component_service.clone();
        let oplog_service = self.state.oplog_service();
        let plugins = self.state.plugins();

        let entry = self.as_wasi_view().table().get(&self_)?.clone();

        let chunk = search_public_oplog(
            component_service,
            oplog_service,
            plugins,
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
                        let entry: OplogEntry = entry.into();
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
    pub next_oplog_index: golem_common::model::oplog::OplogIndex,
    pub current_component_version: ComponentVersion,
    pub page_size: usize,
    pub query: String,
}

impl SearchOplogEntry {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        initial_oplog_index: golem_common::model::oplog::OplogIndex,
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
        next_oplog_index: golem_common::model::oplog::OplogIndex,
        current_component_version: ComponentVersion,
    ) {
        self.next_oplog_index = next_oplog_index;
        self.current_component_version = current_component_version;
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> OplogHost for DurableWorkerCtx<Ctx> {}

impl From<Uuid> for golem_api_0_2_x::host::Uuid {
    fn from(value: Uuid) -> Self {
        golem_api_0_2_x::host::Uuid {
            high_bits: value.high_bits,
            low_bits: value.low_bits,
        }
    }
}

impl From<golem_api_0_2_x::host::Uuid> for Uuid {
    fn from(value: golem_api_0_2_x::host::Uuid) -> Self {
        Uuid {
            high_bits: value.high_bits,
            low_bits: value.low_bits,
        }
    }
}

impl From<ComponentId> for golem_api_0_2_x::host::ComponentId {
    fn from(value: ComponentId) -> Self {
        golem_api_0_2_x::host::ComponentId {
            uuid: value.uuid.into(),
        }
    }
}

impl From<golem_api_0_2_x::host::ComponentId> for ComponentId {
    fn from(value: golem_api_0_2_x::host::ComponentId) -> Self {
        ComponentId {
            uuid: value.uuid.into(),
        }
    }
}

impl From<WorkerId> for golem_api_0_2_x::host::WorkerId {
    fn from(value: WorkerId) -> Self {
        golem_api_0_2_x::host::WorkerId {
            component_id: value.component_id.into(),
            worker_name: value.worker_name,
        }
    }
}

impl From<golem_api_0_2_x::host::WorkerId> for WorkerId {
    fn from(value: golem_api_0_2_x::host::WorkerId) -> Self {
        WorkerId {
            component_id: value.component_id.into(),
            worker_name: value.worker_name,
        }
    }
}

impl From<PromiseId> for golem_api_0_2_x::host::PromiseId {
    fn from(value: PromiseId) -> Self {
        golem_api_0_2_x::host::PromiseId {
            worker_id: value.worker_id.into(),
            oplog_idx: value.oplog_idx,
        }
    }
}

impl From<golem_api_0_2_x::host::PromiseId> for PromiseId {
    fn from(value: golem_api_0_2_x::host::PromiseId) -> Self {
        PromiseId {
            worker_id: value.worker_id.into(),
            oplog_idx: value.oplog_idx,
        }
    }
}

impl From<RetryPolicy> for golem_api_0_2_x::host::RetryPolicy {
    fn from(value: RetryPolicy) -> Self {
        golem_api_0_2_x::host::RetryPolicy {
            max_attempts: value.max_attempts,
            min_delay: value.min_delay,
            max_delay: value.max_delay,
            multiplier: value.multiplier,
        }
    }
}

impl From<golem_api_0_2_x::host::RetryPolicy> for RetryPolicy {
    fn from(value: golem_api_0_2_x::host::RetryPolicy) -> Self {
        RetryPolicy {
            max_attempts: value.max_attempts,
            min_delay: value.min_delay,
            max_delay: value.max_delay,
            multiplier: value.multiplier,
            max_jitter_factor: None,
        }
    }
}

impl From<PersistenceLevel> for golem_api_0_2_x::host::PersistenceLevel {
    fn from(value: PersistenceLevel) -> Self {
        match value {
            PersistenceLevel::PersistNothing => {
                golem_api_0_2_x::host::PersistenceLevel::PersistNothing
            }
            PersistenceLevel::PersistRemoteSideEffects => {
                golem_api_0_2_x::host::PersistenceLevel::PersistRemoteSideEffects
            }
            PersistenceLevel::Smart => golem_api_0_2_x::host::PersistenceLevel::Smart,
        }
    }
}

impl From<golem_api_0_2_x::host::PersistenceLevel> for PersistenceLevel {
    fn from(value: golem_api_0_2_x::host::PersistenceLevel) -> Self {
        match value {
            golem_api_0_2_x::host::PersistenceLevel::PersistNothing => {
                PersistenceLevel::PersistNothing
            }
            golem_api_0_2_x::host::PersistenceLevel::PersistRemoteSideEffects => {
                PersistenceLevel::PersistRemoteSideEffects
            }
            golem_api_0_2_x::host::PersistenceLevel::Smart => PersistenceLevel::Smart,
        }
    }
}

impl From<UpdateMode> for golem_api_0_2_x::host::UpdateMode {
    fn from(value: UpdateMode) -> Self {
        match value {
            UpdateMode::Automatic => golem_api_0_2_x::host::UpdateMode::Automatic,
            UpdateMode::SnapshotBased => golem_api_0_2_x::host::UpdateMode::SnapshotBased,
        }
    }
}

impl From<golem_api_0_2_x::host::UpdateMode> for UpdateMode {
    fn from(value: golem_api_0_2_x::host::UpdateMode) -> Self {
        match value {
            golem_api_0_2_x::host::UpdateMode::Automatic => UpdateMode::Automatic,
            golem_api_0_2_x::host::UpdateMode::SnapshotBased => UpdateMode::SnapshotBased,
        }
    }
}

impl From<WorkerStatus> for golem_api_0_2_x::host::WorkerStatus {
    fn from(value: WorkerStatus) -> Self {
        match value {
            WorkerStatus::Running => golem_api_0_2_x::host::WorkerStatus::Running,
            WorkerStatus::Idle => golem_api_0_2_x::host::WorkerStatus::Idle,
            WorkerStatus::Suspended => golem_api_0_2_x::host::WorkerStatus::Suspended,
            WorkerStatus::Interrupted => golem_api_0_2_x::host::WorkerStatus::Interrupted,
            WorkerStatus::Retrying => golem_api_0_2_x::host::WorkerStatus::Retrying,
            WorkerStatus::Failed => golem_api_0_2_x::host::WorkerStatus::Failed,
            WorkerStatus::Exited => golem_api_0_2_x::host::WorkerStatus::Exited,
        }
    }
}

impl From<golem_api_0_2_x::host::WorkerStatus> for WorkerStatus {
    fn from(value: golem_api_0_2_x::host::WorkerStatus) -> Self {
        match value {
            golem_api_0_2_x::host::WorkerStatus::Running => WorkerStatus::Running,
            golem_api_0_2_x::host::WorkerStatus::Idle => WorkerStatus::Idle,
            golem_api_0_2_x::host::WorkerStatus::Suspended => WorkerStatus::Suspended,
            golem_api_0_2_x::host::WorkerStatus::Interrupted => WorkerStatus::Interrupted,
            golem_api_0_2_x::host::WorkerStatus::Retrying => WorkerStatus::Retrying,
            golem_api_0_2_x::host::WorkerStatus::Failed => WorkerStatus::Failed,
            golem_api_0_2_x::host::WorkerStatus::Exited => WorkerStatus::Exited,
        }
    }
}

impl From<WorkerMetadata> for golem_api_0_2_x::host::WorkerMetadata {
    fn from(value: WorkerMetadata) -> Self {
        golem_api_0_2_x::host::WorkerMetadata {
            worker_id: value.worker_id.into(),
            args: value.args,
            env: value.env,
            status: value.status.into(),
            component_version: value.component_version,
            retry_count: value.retry_count,
        }
    }
}

impl From<golem_api_0_2_x::host::WorkerMetadata> for WorkerMetadata {
    fn from(value: golem_api_0_2_x::host::WorkerMetadata) -> Self {
        WorkerMetadata {
            worker_id: value.worker_id.into(),
            args: value.args,
            env: value.env,
            status: value.status.into(),
            component_version: value.component_version,
            retry_count: value.retry_count,
        }
    }
}

impl From<StringFilterComparator> for golem_api_0_2_x::host::StringFilterComparator {
    fn from(value: StringFilterComparator) -> Self {
        match value {
            StringFilterComparator::Equal => golem_api_0_2_x::host::StringFilterComparator::Equal,
            StringFilterComparator::NotEqual => {
                golem_api_0_2_x::host::StringFilterComparator::NotEqual
            }
            StringFilterComparator::Like => golem_api_0_2_x::host::StringFilterComparator::Like,
            StringFilterComparator::NotLike => {
                golem_api_0_2_x::host::StringFilterComparator::NotLike
            }
        }
    }
}

impl From<FilterComparator> for golem_api_0_2_x::host::FilterComparator {
    fn from(value: FilterComparator) -> Self {
        match value {
            FilterComparator::Equal => golem_api_0_2_x::host::FilterComparator::Equal,
            FilterComparator::NotEqual => golem_api_0_2_x::host::FilterComparator::NotEqual,
            FilterComparator::GreaterEqual => golem_api_0_2_x::host::FilterComparator::GreaterEqual,
            FilterComparator::Greater => golem_api_0_2_x::host::FilterComparator::Greater,
            FilterComparator::LessEqual => golem_api_0_2_x::host::FilterComparator::LessEqual,
            FilterComparator::Less => golem_api_0_2_x::host::FilterComparator::Less,
        }
    }
}

impl From<WorkerNameFilter> for golem_api_0_2_x::host::WorkerNameFilter {
    fn from(value: WorkerNameFilter) -> Self {
        golem_api_0_2_x::host::WorkerNameFilter {
            comparator: value.comparator.into(),
            value: value.value,
        }
    }
}

impl From<WorkerStatusFilter> for golem_api_0_2_x::host::WorkerStatusFilter {
    fn from(value: WorkerStatusFilter) -> Self {
        golem_api_0_2_x::host::WorkerStatusFilter {
            comparator: value.comparator.into(),
            value: value.value.into(),
        }
    }
}

impl From<WorkerVersionFilter> for golem_api_0_2_x::host::WorkerVersionFilter {
    fn from(value: WorkerVersionFilter) -> Self {
        golem_api_0_2_x::host::WorkerVersionFilter {
            comparator: value.comparator.into(),
            value: value.value,
        }
    }
}

impl From<WorkerCreatedAtFilter> for golem_api_0_2_x::host::WorkerCreatedAtFilter {
    fn from(value: WorkerCreatedAtFilter) -> Self {
        golem_api_0_2_x::host::WorkerCreatedAtFilter {
            comparator: value.comparator.into(),
            value: value.value,
        }
    }
}

impl From<WorkerEnvFilter> for golem_api_0_2_x::host::WorkerEnvFilter {
    fn from(value: WorkerEnvFilter) -> Self {
        golem_api_0_2_x::host::WorkerEnvFilter {
            comparator: value.comparator.into(),
            name: value.name,
            value: value.value,
        }
    }
}

impl From<WorkerPropertyFilter> for golem_api_0_2_x::host::WorkerPropertyFilter {
    fn from(value: WorkerPropertyFilter) -> Self {
        match value {
            WorkerPropertyFilter::Name(filter) => {
                golem_api_0_2_x::host::WorkerPropertyFilter::Name(filter.into())
            }
            WorkerPropertyFilter::Status(filter) => {
                golem_api_0_2_x::host::WorkerPropertyFilter::Status(filter.into())
            }
            WorkerPropertyFilter::Version(filter) => {
                golem_api_0_2_x::host::WorkerPropertyFilter::Version(filter.into())
            }
            WorkerPropertyFilter::CreatedAt(filter) => {
                golem_api_0_2_x::host::WorkerPropertyFilter::CreatedAt(filter.into())
            }
            WorkerPropertyFilter::Env(filter) => {
                golem_api_0_2_x::host::WorkerPropertyFilter::Env(filter.into())
            }
        }
    }
}

impl From<WorkerAllFilter> for golem_api_0_2_x::host::WorkerAllFilter {
    fn from(value: WorkerAllFilter) -> Self {
        golem_api_0_2_x::host::WorkerAllFilter {
            filters: value.filters.into_iter().map(|x| x.into()).collect(),
        }
    }
}

impl From<WorkerAnyFilter> for golem_api_0_2_x::host::WorkerAnyFilter {
    fn from(value: WorkerAnyFilter) -> Self {
        golem_api_0_2_x::host::WorkerAnyFilter {
            filters: value.filters.into_iter().map(|x| x.into()).collect(),
        }
    }
}

impl From<golem_common::model::WorkerId> for golem_api_1_x::host::WorkerId {
    fn from(worker_id: golem_common::model::WorkerId) -> Self {
        golem_api_1_x::host::WorkerId {
            component_id: worker_id.component_id.into(),
            worker_name: worker_id.worker_name,
        }
    }
}

impl From<golem_api_1_x::host::WorkerId> for golem_common::model::WorkerId {
    fn from(host: golem_api_1_x::host::WorkerId) -> Self {
        Self {
            component_id: host.component_id.into(),
            worker_name: host.worker_name,
        }
    }
}

impl From<golem_api_1_x::host::ComponentId> for golem_common::model::ComponentId {
    fn from(host: golem_api_1_x::host::ComponentId) -> Self {
        let high_bits = host.uuid.high_bits;
        let low_bits = host.uuid.low_bits;

        Self(uuid::Uuid::from_u64_pair(high_bits, low_bits))
    }
}

impl From<golem_common::model::ComponentId> for golem_api_1_x::host::ComponentId {
    fn from(component_id: golem_common::model::ComponentId) -> Self {
        let (high_bits, low_bits) = component_id.0.as_u64_pair();

        golem_api_1_x::host::ComponentId {
            uuid: golem_api_1_x::host::Uuid {
                high_bits,
                low_bits,
            },
        }
    }
}

impl From<golem_common::model::PromiseId> for golem_api_1_x::host::PromiseId {
    fn from(promise_id: golem_common::model::PromiseId) -> Self {
        golem_api_1_x::host::PromiseId {
            worker_id: promise_id.worker_id.into(),
            oplog_idx: promise_id.oplog_idx.into(),
        }
    }
}

impl From<golem_api_1_x::host::PromiseId> for golem_common::model::PromiseId {
    fn from(host: golem_api_1_x::host::PromiseId) -> Self {
        Self {
            worker_id: host.worker_id.into(),
            oplog_idx: golem_common::model::oplog::OplogIndex::from_u64(host.oplog_idx),
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

impl From<RetryPolicy> for RetryConfig {
    fn from(value: RetryPolicy) -> Self {
        Self {
            max_attempts: value.max_attempts,
            min_delay: Duration::from_nanos(value.min_delay),
            max_delay: Duration::from_nanos(value.max_delay),
            multiplier: value.multiplier,
            max_jitter_factor: value.max_jitter_factor,
        }
    }
}

impl From<uuid::Uuid> for Uuid {
    fn from(uuid: uuid::Uuid) -> Self {
        let (high_bits, low_bits) = uuid.as_u64_pair();
        Uuid {
            high_bits,
            low_bits,
        }
    }
}
