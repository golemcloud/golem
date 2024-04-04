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

use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::config::RetryConfig;
use std::time::Duration;
use tracing::debug;
use uuid::Uuid;
use wasmtime::component::Resource;

use crate::durable_host::serialized::SerializableError;
use crate::durable_host::wasm_rpc::UriExtensions;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::error::GolemError;
use crate::get_oplog_entry;
use crate::metrics::wasm::record_host_function_call;
use crate::model::InterruptKind;
use crate::preview2::golem;
use crate::preview2::golem::api::host::{
    FilterComparator, GetWorkers, HostGetWorkers, OplogIndex, PersistenceLevel, RetryPolicy,
    StringFilterComparator, WorkerFilter, WorkerPropertyFilter, WorkerStatus,
    WorkersMetadataResponse,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{OplogEntry, WrappedFunctionType};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{PromiseId, TemplateId, WorkerId};

#[async_trait]
impl<Ctx: WorkerCtx> HostGetWorkers for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        template_id: golem::api::host::TemplateId,
        filter: WorkerFilter,
        count: u64,
        precise: bool,
    ) -> anyhow::Result<Resource<GetWorkers>> {
        todo!()
    }

    async fn get(
        &mut self,
        self_: Resource<GetWorkers>,
        cursor: u64,
    ) -> anyhow::Result<Result<WorkersMetadataResponse, String>> {
        todo!()
    }

    fn drop(&mut self, rep: Resource<GetWorkers>) -> anyhow::Result<()> {
        todo!()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> golem::api::host::Host for DurableWorkerCtx<Ctx> {
    async fn golem_create_promise(&mut self) -> Result<golem::api::host::PromiseId, anyhow::Error> {
        record_host_function_call("golem::api", "golem_create_promise");
        Ok(self
            .public_state
            .promise_service
            .create(&self.worker_id.worker_id, self.state.oplog_idx)
            .await
            .into())
    }

    async fn golem_await_promise(
        &mut self,
        promise_id: golem::api::host::PromiseId,
    ) -> Result<Vec<u8>, anyhow::Error> {
        record_host_function_call("golem::api", "golem_await_promise");
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

    async fn golem_complete_promise(
        &mut self,
        promise_id: golem::api::host::PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, anyhow::Error> {
        record_host_function_call("golem::api", "golem_complete_promise");
        Durability::<Ctx, bool, SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteLocal,
            "golem_complete_promise",
            |ctx| {
                Box::pin(async move {
                    Ok(ctx
                        .public_state
                        .promise_service
                        .complete(promise_id.into(), data)
                        .await?)
                })
            },
        )
        .await
    }

    async fn golem_delete_promise(
        &mut self,
        promise_id: golem::api::host::PromiseId,
    ) -> Result<(), anyhow::Error> {
        record_host_function_call("golem::api", "golem_delete_promise");
        Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteLocal,
            "golem_delete_promise",
            |ctx| {
                Box::pin(async move {
                    ctx.public_state
                        .promise_service
                        .delete(promise_id.into())
                        .await;
                    Ok(())
                })
            },
        )
        .await
    }

    async fn get_self_uri(
        &mut self,
        function_name: String,
    ) -> Result<golem::rpc::types::Uri, anyhow::Error> {
        record_host_function_call("golem::api", "get_self_uri");
        let uri = golem_wasm_rpc::golem::rpc::types::Uri::golem_uri(
            &self.state.worker_id,
            Some(&function_name),
        );
        Ok(golem::rpc::types::Uri { value: uri.value })
    }

    async fn get_oplog_index(&mut self) -> anyhow::Result<OplogIndex> {
        record_host_function_call("golem::api", "get_oplog_index");
        let result = self.state.oplog_idx;
        if self.state.is_live() {
            self.state.set_oplog_entry(OplogEntry::nop()).await;
        } else {
            let _ = get_oplog_entry!(self.state, OplogEntry::NoOp);
        }
        Ok(result)
    }

    async fn set_oplog_index(&mut self, oplog_idx: OplogIndex) -> anyhow::Result<()> {
        record_host_function_call("golem::api", "set_oplog_index");
        let jump_source = self.state.oplog_idx;
        let jump_target = oplog_idx;
        if jump_target > jump_source {
            Err(anyhow!(
                "Attempted to jump forward in oplog to index {jump_target} from {jump_source}"
            ))
        } else if self.state.deleted_regions.is_in_deleted_region(jump_target) {
            Err(anyhow!(
                "Attempted to jump to a deleted region in oplog to index {jump_target} from {jump_source}"
            ))
        } else if self.state.is_live() {
            let jump = OplogRegion {
                start: jump_target,
                end: jump_source,
            };

            // Write an oplog entry with the new jump and then restart the worker
            self.state.deleted_regions.add(jump.clone());
            self.state.set_oplog_entry(OplogEntry::jump(jump)).await;
            self.state.commit_oplog().await;

            debug!(
                "Interrupting live execution of {} for jumping from {jump_source} to {jump_target}",
                self.worker_id
            );
            Err(InterruptKind::Jump.into())
        } else {
            // In replay mode we never have to do anything here
            debug!("Ignoring replayed set_oplog_index for {}", self.worker_id);
            Ok(())
        }
    }

    async fn oplog_commit(&mut self, replicas: u8) -> anyhow::Result<()> {
        if self.state.is_live() {
            let timeout = Duration::from_secs(1);
            debug!(
                "Worker {} committing oplog to {} replicas",
                self.worker_id, replicas
            );
            loop {
                // Applying a timeout to make sure the worker remains interruptible
                if self.state.commit_oplog_to_replicas(replicas, timeout).await {
                    debug!(
                        "Worker {} committed oplog to {} replicas",
                        self.worker_id, replicas
                    );
                    return Ok(());
                } else {
                    debug!(
                        "Worker {} failed to commit oplog to {} replicas, retrying",
                        self.worker_id, replicas
                    );
                }

                if let Some(kind) = self.check_interrupt() {
                    return Err(kind.into());
                }
            }
        } else {
            Ok(())
        }
    }

    async fn mark_begin_operation(&mut self) -> anyhow::Result<OplogIndex> {
        record_host_function_call("golem::api", "mark_begin_operation");
        let begin_index = self.state.oplog_idx;

        self.state.consume_hint_entries().await;
        if self.state.is_live() {
            self.state
                .set_oplog_entry(OplogEntry::begin_atomic_region())
                .await;
        } else {
            let _ = get_oplog_entry!(self.state, OplogEntry::BeginAtomicRegion)?;

            match self
                .state
                .lookup_oplog_entry(begin_index, OplogEntry::is_end_atomic_region)
                .await
            {
                Some(end_index) => {
                    debug!(
                        "Worker {}'s atomic operation starting at {} is already committed at {}",
                        self.worker_id, begin_index, end_index
                    );
                }
                None => {
                    debug!("Worker {}'s atomic operation starting at {} is not committed, ignoring persisted entries", self.worker_id, begin_index);

                    // We need to jump to the end of the oplog
                    self.state.oplog_idx = self.state.oplog_size;

                    // But this is not enough, because if the retried transactional block succeeds,
                    // and later we replay it, we need to skip the first attempt and only replay the second.
                    // Se we add a Jump entry to the oplog that registers a deleted region.
                    let deleted_region = OplogRegion {
                        start: begin_index + 1,         // need to keep the BeginAtomicRegion entry
                        end: self.state.oplog_size + 1, // skipping the Jump entry too
                    };
                    self.state.deleted_regions.add(deleted_region.clone());
                    self.state
                        .set_oplog_entry(OplogEntry::jump(deleted_region))
                        .await;
                    self.state.commit_oplog().await;
                }
            }
        }
        Ok(begin_index)
    }

    async fn mark_end_operation(&mut self, begin: OplogIndex) -> anyhow::Result<()> {
        record_host_function_call("golem::api", "mark_end_operation");
        self.state.consume_hint_entries().await;
        if self.state.is_live() {
            self.state
                .set_oplog_entry(OplogEntry::end_atomic_region(begin))
                .await;
        } else {
            let _ = get_oplog_entry!(self.state, OplogEntry::EndAtomicRegion)?;
        }

        Ok(())
    }

    async fn get_retry_policy(&mut self) -> anyhow::Result<RetryPolicy> {
        record_host_function_call("golem::api", "get_retry_policy");
        match &self.state.overridden_retry_policy {
            Some(policy) => Ok(policy.into()),
            None => Ok((&self.state.config.retry).into()),
        }
    }

    async fn set_retry_policy(&mut self, new_retry_policy: RetryPolicy) -> anyhow::Result<()> {
        record_host_function_call("golem::api", "set_retry_policy");
        let new_retry_policy: RetryConfig = new_retry_policy.into();
        self.state.overridden_retry_policy = Some(new_retry_policy.clone());

        self.state.consume_hint_entries().await;
        if self.state.is_live() {
            self.state
                .set_oplog_entry(OplogEntry::change_retry_policy(new_retry_policy))
                .await;
        } else {
            let _ = get_oplog_entry!(self.state, OplogEntry::ChangeRetryPolicy)?;
        }
        Ok(())
    }

    async fn get_oplog_persistence_level(&mut self) -> anyhow::Result<PersistenceLevel> {
        record_host_function_call("golem::api", "get_oplog_persistence_level");
        Ok(self.state.persistence_level.clone().into())
    }

    async fn set_oplog_persistence_level(
        &mut self,
        new_persistence_level: PersistenceLevel,
    ) -> anyhow::Result<()> {
        record_host_function_call("golem::api", "set_oplog_persistence_level");
        // commit all pending entries and change persistence level
        if self.state.is_live() {
            self.state.commit_oplog().await;
        }
        self.state.persistence_level = new_persistence_level.into();
        debug!(
            "Worker {}'s oplog persistence level is set to {:?}",
            self.worker_id, self.state.persistence_level
        );
        Ok(())
    }

    async fn get_idempotence_mode(&mut self) -> anyhow::Result<bool> {
        record_host_function_call("golem::api", "get_idempotence_mode");
        Ok(self.state.assume_idempotence)
    }

    async fn set_idempotence_mode(&mut self, idempotent: bool) -> anyhow::Result<()> {
        record_host_function_call("golem::api", "set_idempotence_mode");
        self.state.assume_idempotence = idempotent;
        Ok(())
    }

    async fn generate_idempotency_key(&mut self) -> anyhow::Result<golem::api::host::Uuid> {
        record_host_function_call("golem::api", "generate_idempotency_key");
        let uuid = Durability::<Ctx, (u64, u64), SerializableError>::custom_wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem api::generate_idempotency_key",
            |_ctx| {
                Box::pin(async move {
                    let uuid = Uuid::new_v4();
                    Ok::<Uuid, GolemError>(uuid)
                })
            },
            |_ctx, uuid: &Uuid| Ok(uuid.as_u64_pair()),
            |_ctx, (high_bits, low_bits)| {
                Box::pin(async move { Ok(Uuid::from_u64_pair(high_bits, low_bits)) })
            },
        )
        .await?;
        Ok(uuid.into())
    }
}

impl From<WorkerId> for golem::api::host::WorkerId {
    fn from(worker_id: WorkerId) -> Self {
        golem::api::host::WorkerId {
            template_id: worker_id.template_id.into(),
            worker_name: worker_id.worker_name,
        }
    }
}

impl From<golem::api::host::WorkerId> for WorkerId {
    fn from(host: golem::api::host::WorkerId) -> Self {
        Self {
            template_id: host.template_id.into(),
            worker_name: host.worker_name,
        }
    }
}

impl From<golem::api::host::TemplateId> for TemplateId {
    fn from(host: golem::api::host::TemplateId) -> Self {
        let high_bits = host.uuid.high_bits;
        let low_bits = host.uuid.low_bits;

        Self(Uuid::from_u64_pair(high_bits, low_bits))
    }
}

impl From<TemplateId> for golem::api::host::TemplateId {
    fn from(template_id: TemplateId) -> Self {
        let (high_bits, low_bits) = template_id.0.as_u64_pair();

        golem::api::host::TemplateId {
            uuid: golem::api::host::Uuid {
                high_bits,
                low_bits,
            },
        }
    }
}

impl From<PromiseId> for golem::api::host::PromiseId {
    fn from(promise_id: PromiseId) -> Self {
        golem::api::host::PromiseId {
            worker_id: promise_id.worker_id.into(),
            oplog_idx: promise_id.oplog_idx,
        }
    }
}

impl From<golem::api::host::PromiseId> for PromiseId {
    fn from(host: golem::api::host::PromiseId) -> Self {
        Self {
            worker_id: host.worker_id.into(),
            oplog_idx: host.oplog_idx,
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
        }
    }
}

impl From<Uuid> for golem::api::host::Uuid {
    fn from(uuid: Uuid) -> Self {
        let (high_bits, low_bits) = uuid.as_u64_pair();
        golem::api::host::Uuid {
            high_bits,
            low_bits,
        }
    }
}

impl From<FilterComparator> for golem_common::model::FilterComparator {
    fn from(value: FilterComparator) -> Self {
        match value {
            FilterComparator::Equal => golem_common::model::FilterComparator::Equal,
            FilterComparator::NotEqual => golem_common::model::FilterComparator::NotEqual,
            FilterComparator::Less => golem_common::model::FilterComparator::Less,
            FilterComparator::LessEqual => golem_common::model::FilterComparator::LessEqual,
            FilterComparator::Greater => golem_common::model::FilterComparator::Greater,
            FilterComparator::GreaterEqual => golem_common::model::FilterComparator::GreaterEqual,
        }
    }
}

impl From<StringFilterComparator> for golem_common::model::StringFilterComparator {
    fn from(value: StringFilterComparator) -> Self {
        match value {
            StringFilterComparator::Equal => golem_common::model::StringFilterComparator::Equal,
            StringFilterComparator::NotEqual => {
                golem_common::model::StringFilterComparator::NotEqual
            }
            StringFilterComparator::Like => golem_common::model::StringFilterComparator::Like,
            StringFilterComparator::NotLike => golem_common::model::StringFilterComparator::NotLike,
        }
    }
}

impl From<WorkerStatus> for golem_common::model::WorkerStatus {
    fn from(value: WorkerStatus) -> Self {
        match value {
            WorkerStatus::Running => golem_common::model::WorkerStatus::Running,
            WorkerStatus::Idle => golem_common::model::WorkerStatus::Idle,
            WorkerStatus::Suspended => golem_common::model::WorkerStatus::Suspended,
            WorkerStatus::Interrupted => golem_common::model::WorkerStatus::Interrupted,
            WorkerStatus::Retrying => golem_common::model::WorkerStatus::Retrying,
            WorkerStatus::Failed => golem_common::model::WorkerStatus::Failed,
            WorkerStatus::Exited => golem_common::model::WorkerStatus::Exited,
        }
    }
}

impl From<WorkerPropertyFilter> for golem_common::model::WorkerFilter {
    fn from(filter: WorkerPropertyFilter) -> Self {
        match filter {
            WorkerPropertyFilter::Name(filter) => {
                golem_common::model::WorkerFilter::new_name(filter.comparator.into(), filter.value)
            }
            WorkerPropertyFilter::Version(filter) => {
                golem_common::model::WorkerFilter::new_version(
                    filter.comparator.into(),
                    filter.value,
                )
            }
            WorkerPropertyFilter::Status(filter) => golem_common::model::WorkerFilter::new_status(
                filter.comparator.into(),
                filter.value.into(),
            ),
            WorkerPropertyFilter::Env(filter) => golem_common::model::WorkerFilter::new_env(
                filter.name,
                filter.comparator.into(),
                filter.value,
            ),
            WorkerPropertyFilter::CreatedAt(filter) => {
                golem_common::model::WorkerFilter::new_created_at(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
        }
    }
}

impl From<WorkerFilter> for golem_common::model::WorkerFilter {
    fn from(filter: WorkerFilter) -> Self {
        match filter {
            WorkerFilter::And(filters) => {
                let filters = filters.into_iter().map(|f| f.into()).collect();
                golem_common::model::WorkerFilter::new_and(filters)
            }
            WorkerFilter::Or(filters) => {
                let filters = filters.into_iter().map(|f| f.into()).collect();
                golem_common::model::WorkerFilter::new_or(filters)
            }
        }
    }
}
