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

use crate::durable_host::golem::GetWorkersEntry;
use crate::durable_host::DurableWorkerCtx;
use crate::preview2::golem;
use crate::preview2::golem::api0_2_0::host::GetWorkers;
use crate::preview2::golem::api1_1_0_rc1::host::{
    ComponentId, ComponentVersion, FilterComparator, Host, HostGetWorkers, OplogIndex,
    PersistenceLevel, PromiseId, RetryPolicy, StringFilterComparator, UpdateMode, Uuid,
    WorkerAllFilter, WorkerAnyFilter, WorkerCreatedAtFilter, WorkerEnvFilter, WorkerId,
    WorkerMetadata, WorkerNameFilter, WorkerPropertyFilter, WorkerStatus, WorkerStatusFilter,
    WorkerVersionFilter,
};
use crate::preview2::golem::api1_1_0_rc1::oplog::{
    GetOplog, Host as OplogHost, HostGetOplog, OplogEntry,
};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use wasmtime::component::Resource;

#[async_trait]
impl<Ctx: WorkerCtx> HostGetWorkers for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        component_id: ComponentId,
        filter: Option<WorkerAnyFilter>,
        precise: bool,
    ) -> anyhow::Result<Resource<GetWorkers>> {
        golem::api0_2_0::host::HostGetWorkers::new(
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
        golem::api0_2_0::host::HostGetWorkers::get_next(self, self_)
            .await
            .map(|x| x.map(|x| x.into_iter().map(|x| x.into()).collect()))
    }

    fn drop(&mut self, rep: Resource<GetWorkers>) -> anyhow::Result<()> {
        golem::api0_2_0::host::HostGetWorkers::drop(self, rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn create_promise(&mut self) -> anyhow::Result<PromiseId> {
        golem::api0_2_0::host::Host::create_promise(self)
            .await
            .map(|x| x.into())
    }

    async fn await_promise(&mut self, promise_id: PromiseId) -> anyhow::Result<Vec<u8>> {
        golem::api0_2_0::host::Host::await_promise(self, promise_id.into()).await
    }

    async fn complete_promise(
        &mut self,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> anyhow::Result<bool> {
        golem::api0_2_0::host::Host::complete_promise(self, promise_id.into(), data).await
    }

    async fn delete_promise(&mut self, promise_id: PromiseId) -> anyhow::Result<()> {
        golem::api0_2_0::host::Host::delete_promise(self, promise_id.into()).await
    }

    async fn get_oplog_index(&mut self) -> anyhow::Result<OplogIndex> {
        golem::api0_2_0::host::Host::get_oplog_index(self).await
    }

    async fn set_oplog_index(&mut self, oplog_idx: OplogIndex) -> anyhow::Result<()> {
        golem::api0_2_0::host::Host::set_oplog_index(self, oplog_idx).await
    }

    async fn oplog_commit(&mut self, replicas: u8) -> anyhow::Result<()> {
        golem::api0_2_0::host::Host::oplog_commit(self, replicas).await
    }

    async fn mark_begin_operation(&mut self) -> anyhow::Result<OplogIndex> {
        golem::api0_2_0::host::Host::mark_begin_operation(self).await
    }

    async fn mark_end_operation(&mut self, begin: OplogIndex) -> anyhow::Result<()> {
        golem::api0_2_0::host::Host::mark_end_operation(self, begin).await
    }

    async fn get_retry_policy(&mut self) -> anyhow::Result<RetryPolicy> {
        golem::api0_2_0::host::Host::get_retry_policy(self)
            .await
            .map(|x| x.into())
    }

    async fn set_retry_policy(&mut self, new_retry_policy: RetryPolicy) -> anyhow::Result<()> {
        golem::api0_2_0::host::Host::set_retry_policy(self, new_retry_policy.into()).await
    }

    async fn get_oplog_persistence_level(&mut self) -> anyhow::Result<PersistenceLevel> {
        golem::api0_2_0::host::Host::get_oplog_persistence_level(self)
            .await
            .map(|x| x.into())
    }

    async fn set_oplog_persistence_level(
        &mut self,
        new_persistence_level: PersistenceLevel,
    ) -> anyhow::Result<()> {
        golem::api0_2_0::host::Host::set_oplog_persistence_level(self, new_persistence_level.into())
            .await
    }

    async fn get_idempotence_mode(&mut self) -> anyhow::Result<bool> {
        golem::api0_2_0::host::Host::get_idempotence_mode(self).await
    }

    async fn set_idempotence_mode(&mut self, idempotent: bool) -> anyhow::Result<()> {
        golem::api0_2_0::host::Host::set_idempotence_mode(self, idempotent).await
    }

    async fn generate_idempotency_key(&mut self) -> anyhow::Result<Uuid> {
        golem::api0_2_0::host::Host::generate_idempotency_key(self)
            .await
            .map(|x| x.into())
    }

    async fn update_worker(
        &mut self,
        worker_id: WorkerId,
        target_version: ComponentVersion,
        mode: UpdateMode,
    ) -> anyhow::Result<()> {
        golem::api0_2_0::host::Host::update_worker(
            self,
            worker_id.into(),
            target_version,
            mode.into(),
        )
        .await
    }

    async fn get_self_metadata(&mut self) -> anyhow::Result<WorkerMetadata> {
        golem::api0_2_0::host::Host::get_self_metadata(self)
            .await
            .map(|x| x.into())
    }

    async fn get_worker_metadata(
        &mut self,
        worker_id: WorkerId,
    ) -> anyhow::Result<Option<WorkerMetadata>> {
        golem::api0_2_0::host::Host::get_worker_metadata(self, worker_id.into())
            .await
            .map(|x| x.map(|x| x.into()))
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostGetOplog for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        worker_id: crate::preview2::golem::api1_1_0_rc1::oplog::WorkerId,
        start: crate::preview2::golem::api1_1_0_rc1::oplog::OplogIndex,
    ) -> anyhow::Result<Resource<GetOplog>> {
        todo!()
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetOplog>,
    ) -> anyhow::Result<Option<Vec<OplogEntry>>> {
        todo!()
    }

    fn drop(&mut self, rep: Resource<GetOplog>) -> anyhow::Result<()> {
        todo!()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> OplogHost for DurableWorkerCtx<Ctx> {}

#[async_trait]
impl<Ctx: WorkerCtx> HostGetWorkers for &mut DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        component_id: ComponentId,
        filter: Option<WorkerAnyFilter>,
        precise: bool,
    ) -> anyhow::Result<Resource<GetWorkersEntry>> {
        HostGetWorkers::new(*self, component_id, filter, precise).await
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetWorkersEntry>,
    ) -> anyhow::Result<Option<Vec<WorkerMetadata>>> {
        HostGetWorkers::get_next(*self, self_).await
    }

    fn drop(&mut self, rep: Resource<GetWorkersEntry>) -> anyhow::Result<()> {
        HostGetWorkers::drop(*self, rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {
    async fn create_promise(&mut self) -> anyhow::Result<PromiseId> {
        (*self).create_promise().await
    }

    async fn await_promise(&mut self, promise_id: PromiseId) -> anyhow::Result<Vec<u8>> {
        (*self).await_promise(promise_id).await
    }

    async fn complete_promise(
        &mut self,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> anyhow::Result<bool> {
        (*self).complete_promise(promise_id, data).await
    }

    async fn delete_promise(&mut self, promise_id: PromiseId) -> anyhow::Result<()> {
        (*self).delete_promise(promise_id).await
    }

    async fn get_oplog_index(&mut self) -> anyhow::Result<OplogIndex> {
        (*self).get_oplog_index().await
    }

    async fn set_oplog_index(&mut self, oplog_idx: OplogIndex) -> anyhow::Result<()> {
        (*self).set_oplog_index(oplog_idx).await
    }

    async fn oplog_commit(&mut self, replicas: u8) -> anyhow::Result<()> {
        (*self).oplog_commit(replicas).await
    }

    async fn mark_begin_operation(&mut self) -> anyhow::Result<OplogIndex> {
        (*self).mark_begin_operation().await
    }

    async fn mark_end_operation(&mut self, begin: OplogIndex) -> anyhow::Result<()> {
        (*self).mark_end_operation(begin).await
    }

    async fn get_retry_policy(&mut self) -> anyhow::Result<RetryPolicy> {
        (*self).get_retry_policy().await
    }

    async fn set_retry_policy(&mut self, new_retry_policy: RetryPolicy) -> anyhow::Result<()> {
        (*self).set_retry_policy(new_retry_policy).await
    }

    async fn get_oplog_persistence_level(&mut self) -> anyhow::Result<PersistenceLevel> {
        (*self).get_oplog_persistence_level().await
    }

    async fn set_oplog_persistence_level(
        &mut self,
        new_persistence_level: PersistenceLevel,
    ) -> anyhow::Result<()> {
        (*self)
            .set_oplog_persistence_level(new_persistence_level)
            .await
    }

    async fn get_idempotence_mode(&mut self) -> anyhow::Result<bool> {
        (*self).get_idempotence_mode().await
    }

    async fn set_idempotence_mode(&mut self, idempotent: bool) -> anyhow::Result<()> {
        (*self).set_idempotence_mode(idempotent).await
    }

    async fn generate_idempotency_key(&mut self) -> anyhow::Result<Uuid> {
        (*self).generate_idempotency_key().await
    }

    async fn update_worker(
        &mut self,
        worker_id: WorkerId,
        target_version: ComponentVersion,
        mode: UpdateMode,
    ) -> anyhow::Result<()> {
        (*self).update_worker(worker_id, target_version, mode).await
    }

    async fn get_self_metadata(&mut self) -> anyhow::Result<WorkerMetadata> {
        (*self).get_self_metadata().await
    }

    async fn get_worker_metadata(
        &mut self,
        worker_id: WorkerId,
    ) -> anyhow::Result<Option<WorkerMetadata>> {
        (*self).get_worker_metadata(worker_id).await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostGetOplog for &mut DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        worker_id: golem::api1_1_0_rc1::oplog::WorkerId,
        start: golem::api1_1_0_rc1::oplog::OplogIndex,
    ) -> anyhow::Result<Resource<GetOplog>> {
        HostGetOplog::new(*self, worker_id, start).await
    }

    async fn get_next(
        &mut self,
        self_: Resource<GetOplog>,
    ) -> anyhow::Result<Option<Vec<OplogEntry>>> {
        HostGetOplog::get_next(*self, self_).await
    }

    fn drop(&mut self, rep: Resource<GetOplog>) -> anyhow::Result<()> {
        HostGetOplog::drop(*self, rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> OplogHost for &mut DurableWorkerCtx<Ctx> {}

impl From<Uuid> for golem::api0_2_0::host::Uuid {
    fn from(value: Uuid) -> Self {
        golem::api0_2_0::host::Uuid {
            high_bits: value.high_bits,
            low_bits: value.low_bits,
        }
    }
}

impl From<golem::api0_2_0::host::Uuid> for Uuid {
    fn from(value: golem::api0_2_0::host::Uuid) -> Self {
        Uuid {
            high_bits: value.high_bits,
            low_bits: value.low_bits,
        }
    }
}

impl From<ComponentId> for golem::api0_2_0::host::ComponentId {
    fn from(value: ComponentId) -> Self {
        golem::api0_2_0::host::ComponentId {
            uuid: value.uuid.into(),
        }
    }
}

impl From<golem::api0_2_0::host::ComponentId> for ComponentId {
    fn from(value: golem::api0_2_0::host::ComponentId) -> Self {
        ComponentId {
            uuid: value.uuid.into(),
        }
    }
}

impl From<WorkerId> for golem::api0_2_0::host::WorkerId {
    fn from(value: WorkerId) -> Self {
        golem::api0_2_0::host::WorkerId {
            component_id: value.component_id.into(),
            worker_name: value.worker_name,
        }
    }
}

impl From<golem::api0_2_0::host::WorkerId> for WorkerId {
    fn from(value: golem::api0_2_0::host::WorkerId) -> Self {
        WorkerId {
            component_id: value.component_id.into(),
            worker_name: value.worker_name,
        }
    }
}

impl From<PromiseId> for golem::api0_2_0::host::PromiseId {
    fn from(value: PromiseId) -> Self {
        golem::api0_2_0::host::PromiseId {
            worker_id: value.worker_id.into(),
            oplog_idx: value.oplog_idx,
        }
    }
}

impl From<golem::api0_2_0::host::PromiseId> for PromiseId {
    fn from(value: golem::api0_2_0::host::PromiseId) -> Self {
        PromiseId {
            worker_id: value.worker_id.into(),
            oplog_idx: value.oplog_idx,
        }
    }
}

impl From<RetryPolicy> for golem::api0_2_0::host::RetryPolicy {
    fn from(value: RetryPolicy) -> Self {
        golem::api0_2_0::host::RetryPolicy {
            max_attempts: value.max_attempts,
            min_delay: value.min_delay,
            max_delay: value.max_delay,
            multiplier: value.multiplier,
        }
    }
}

impl From<golem::api0_2_0::host::RetryPolicy> for RetryPolicy {
    fn from(value: golem::api0_2_0::host::RetryPolicy) -> Self {
        RetryPolicy {
            max_attempts: value.max_attempts,
            min_delay: value.min_delay,
            max_delay: value.max_delay,
            multiplier: value.multiplier,
            max_jitter_factory: None,
        }
    }
}

impl From<PersistenceLevel> for golem::api0_2_0::host::PersistenceLevel {
    fn from(value: PersistenceLevel) -> Self {
        match value {
            PersistenceLevel::PersistNothing => {
                golem::api0_2_0::host::PersistenceLevel::PersistNothing
            }
            PersistenceLevel::PersistRemoteSideEffects => {
                golem::api0_2_0::host::PersistenceLevel::PersistRemoteSideEffects
            }
            PersistenceLevel::Smart => golem::api0_2_0::host::PersistenceLevel::Smart,
        }
    }
}

impl From<golem::api0_2_0::host::PersistenceLevel> for PersistenceLevel {
    fn from(value: golem::api0_2_0::host::PersistenceLevel) -> Self {
        match value {
            golem::api0_2_0::host::PersistenceLevel::PersistNothing => {
                PersistenceLevel::PersistNothing
            }
            golem::api0_2_0::host::PersistenceLevel::PersistRemoteSideEffects => {
                PersistenceLevel::PersistRemoteSideEffects
            }
            golem::api0_2_0::host::PersistenceLevel::Smart => PersistenceLevel::Smart,
        }
    }
}

impl From<UpdateMode> for golem::api0_2_0::host::UpdateMode {
    fn from(value: UpdateMode) -> Self {
        match value {
            UpdateMode::Automatic => golem::api0_2_0::host::UpdateMode::Automatic,
            UpdateMode::SnapshotBased => golem::api0_2_0::host::UpdateMode::SnapshotBased,
        }
    }
}

impl From<golem::api0_2_0::host::UpdateMode> for UpdateMode {
    fn from(value: golem::api0_2_0::host::UpdateMode) -> Self {
        match value {
            golem::api0_2_0::host::UpdateMode::Automatic => UpdateMode::Automatic,
            golem::api0_2_0::host::UpdateMode::SnapshotBased => UpdateMode::SnapshotBased,
        }
    }
}

impl From<WorkerStatus> for golem::api0_2_0::host::WorkerStatus {
    fn from(value: WorkerStatus) -> Self {
        match value {
            WorkerStatus::Running => golem::api0_2_0::host::WorkerStatus::Running,
            WorkerStatus::Idle => golem::api0_2_0::host::WorkerStatus::Idle,
            WorkerStatus::Suspended => golem::api0_2_0::host::WorkerStatus::Suspended,
            WorkerStatus::Interrupted => golem::api0_2_0::host::WorkerStatus::Interrupted,
            WorkerStatus::Retrying => golem::api0_2_0::host::WorkerStatus::Retrying,
            WorkerStatus::Failed => golem::api0_2_0::host::WorkerStatus::Failed,
            WorkerStatus::Exited => golem::api0_2_0::host::WorkerStatus::Exited,
        }
    }
}

impl From<golem::api0_2_0::host::WorkerStatus> for WorkerStatus {
    fn from(value: golem::api0_2_0::host::WorkerStatus) -> Self {
        match value {
            golem::api0_2_0::host::WorkerStatus::Running => WorkerStatus::Running,
            golem::api0_2_0::host::WorkerStatus::Idle => WorkerStatus::Idle,
            golem::api0_2_0::host::WorkerStatus::Suspended => WorkerStatus::Suspended,
            golem::api0_2_0::host::WorkerStatus::Interrupted => WorkerStatus::Interrupted,
            golem::api0_2_0::host::WorkerStatus::Retrying => WorkerStatus::Retrying,
            golem::api0_2_0::host::WorkerStatus::Failed => WorkerStatus::Failed,
            golem::api0_2_0::host::WorkerStatus::Exited => WorkerStatus::Exited,
        }
    }
}

impl From<WorkerMetadata> for golem::api0_2_0::host::WorkerMetadata {
    fn from(value: WorkerMetadata) -> Self {
        golem::api0_2_0::host::WorkerMetadata {
            worker_id: value.worker_id.into(),
            args: value.args,
            env: value.env,
            status: value.status.into(),
            component_version: value.component_version,
            retry_count: value.retry_count,
        }
    }
}

impl From<golem::api0_2_0::host::WorkerMetadata> for WorkerMetadata {
    fn from(value: golem::api0_2_0::host::WorkerMetadata) -> Self {
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

impl From<StringFilterComparator> for golem::api0_2_0::host::StringFilterComparator {
    fn from(value: StringFilterComparator) -> Self {
        match value {
            StringFilterComparator::Equal => golem::api0_2_0::host::StringFilterComparator::Equal,
            StringFilterComparator::NotEqual => {
                golem::api0_2_0::host::StringFilterComparator::NotEqual
            }
            StringFilterComparator::Like => golem::api0_2_0::host::StringFilterComparator::Like,
            StringFilterComparator::NotLike => {
                golem::api0_2_0::host::StringFilterComparator::NotLike
            }
        }
    }
}

impl From<FilterComparator> for golem::api0_2_0::host::FilterComparator {
    fn from(value: FilterComparator) -> Self {
        match value {
            FilterComparator::Equal => golem::api0_2_0::host::FilterComparator::Equal,
            FilterComparator::NotEqual => golem::api0_2_0::host::FilterComparator::NotEqual,
            FilterComparator::GreaterEqual => golem::api0_2_0::host::FilterComparator::GreaterEqual,
            FilterComparator::Greater => golem::api0_2_0::host::FilterComparator::Greater,
            FilterComparator::LessEqual => golem::api0_2_0::host::FilterComparator::LessEqual,
            FilterComparator::Less => golem::api0_2_0::host::FilterComparator::Less,
        }
    }
}

impl From<WorkerNameFilter> for golem::api0_2_0::host::WorkerNameFilter {
    fn from(value: WorkerNameFilter) -> Self {
        golem::api0_2_0::host::WorkerNameFilter {
            comparator: value.comparator.into(),
            value: value.value,
        }
    }
}

impl From<WorkerStatusFilter> for golem::api0_2_0::host::WorkerStatusFilter {
    fn from(value: WorkerStatusFilter) -> Self {
        golem::api0_2_0::host::WorkerStatusFilter {
            comparator: value.comparator.into(),
            value: value.value.into(),
        }
    }
}

impl From<WorkerVersionFilter> for golem::api0_2_0::host::WorkerVersionFilter {
    fn from(value: WorkerVersionFilter) -> Self {
        golem::api0_2_0::host::WorkerVersionFilter {
            comparator: value.comparator.into(),
            value: value.value,
        }
    }
}

impl From<WorkerCreatedAtFilter> for golem::api0_2_0::host::WorkerCreatedAtFilter {
    fn from(value: WorkerCreatedAtFilter) -> Self {
        golem::api0_2_0::host::WorkerCreatedAtFilter {
            comparator: value.comparator.into(),
            value: value.value,
        }
    }
}

impl From<WorkerEnvFilter> for golem::api0_2_0::host::WorkerEnvFilter {
    fn from(value: WorkerEnvFilter) -> Self {
        golem::api0_2_0::host::WorkerEnvFilter {
            comparator: value.comparator.into(),
            name: value.name,
            value: value.value,
        }
    }
}

impl From<WorkerPropertyFilter> for golem::api0_2_0::host::WorkerPropertyFilter {
    fn from(value: WorkerPropertyFilter) -> Self {
        match value {
            WorkerPropertyFilter::Name(filter) => {
                golem::api0_2_0::host::WorkerPropertyFilter::Name(filter.into())
            }
            WorkerPropertyFilter::Status(filter) => {
                golem::api0_2_0::host::WorkerPropertyFilter::Status(filter.into())
            }
            WorkerPropertyFilter::Version(filter) => {
                golem::api0_2_0::host::WorkerPropertyFilter::Version(filter.into())
            }
            WorkerPropertyFilter::CreatedAt(filter) => {
                golem::api0_2_0::host::WorkerPropertyFilter::CreatedAt(filter.into())
            }
            WorkerPropertyFilter::Env(filter) => {
                golem::api0_2_0::host::WorkerPropertyFilter::Env(filter.into())
            }
        }
    }
}

impl From<WorkerAllFilter> for golem::api0_2_0::host::WorkerAllFilter {
    fn from(value: WorkerAllFilter) -> Self {
        golem::api0_2_0::host::WorkerAllFilter {
            filters: value.filters.into_iter().map(|x| x.into()).collect(),
        }
    }
}

impl From<WorkerAnyFilter> for golem::api0_2_0::host::WorkerAnyFilter {
    fn from(value: WorkerAnyFilter) -> Self {
        golem::api0_2_0::host::WorkerAnyFilter {
            filters: value.filters.into_iter().map(|x| x.into()).collect(),
        }
    }
}
