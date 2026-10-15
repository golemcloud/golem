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

use super::{AdditionalTestDeps, TestOplog};
use async_trait::async_trait;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogEntry, OplogIndex, PayloadId, RawOplogPayload};
use golem_common::model::{
    AgentId, AgentMetadata, AgentStatusRecord, OwnedAgentId, ScanCursor, ShardEpoch,
};
use golem_common::read_only_lock;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_worker_executor::model::ExecutionStatus;
use golem_worker_executor::services::oplog::{
    Oplog, OplogError, OplogLifecycleGuard, OplogService, RawOplogPayloadDownloadError,
    StagePublication,
};
use golem_worker_executor::services::stream_session_index::StreamSessionIndexService;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

pub(super) struct OplogFixtureService {
    inner: Arc<dyn OplogService>,
    deps: AdditionalTestDeps,
}

impl OplogFixtureService {
    pub(super) fn new(inner: Arc<dyn OplogService>, deps: AdditionalTestDeps) -> Self {
        Self { inner, deps }
    }

    fn handle(&self, id: &OwnedAgentId, inner: Arc<dyn Oplog>) -> Arc<dyn Oplog> {
        Arc::new(TestOplog::new(id.clone(), inner, self.deps.clone()))
    }
}

impl Debug for OplogFixtureService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OplogFixtureService")
            .field("inner", &self.inner)
            .finish()
    }
}

#[async_trait]
impl OplogService for OplogFixtureService {
    async fn lock_lifecycle(&self, id: &AgentId) -> OplogLifecycleGuard {
        self.inner.lock_lifecycle(id).await
    }

    fn set_stream_session_index(&self, index: Arc<StreamSessionIndexService>) {
        self.inner.set_stream_session_index(index)
    }

    fn stream_session_index(&self) -> Option<Arc<StreamSessionIndexService>> {
        self.inner.stream_session_index()
    }

    async fn create_staged(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        stage: uuid::Uuid,
        metadata: AgentMetadata,
    ) -> Result<Arc<dyn Oplog>, String> {
        Ok(self.handle(
            id,
            self.inner.create_staged(id, mode, stage, metadata).await?,
        ))
    }

    async fn staged_exists(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        stage: uuid::Uuid,
    ) -> Result<bool, String> {
        self.inner.staged_exists(id, mode, stage).await
    }

    async fn publish_staged(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        publication: StagePublication,
        last: OplogIndex,
    ) -> Result<bool, String> {
        self.inner.publish_staged(id, mode, publication, last).await
    }

    async fn discard_staged(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        stage: uuid::Uuid,
    ) -> Result<(), String> {
        self.inner.discard_staged(id, mode, stage).await
    }

    async fn create(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        id: &OwnedAgentId,
        mode: AgentMode,
        initial: OplogEntry,
        metadata: AgentMetadata,
        status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        epoch: Option<ShardEpoch>,
    ) -> Arc<dyn Oplog> {
        self.handle(
            id,
            self.inner
                .create(
                    lifecycle, id, mode, initial, metadata, status, execution, epoch,
                )
                .await,
        )
    }

    async fn create_fresh(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        id: &OwnedAgentId,
        mode: AgentMode,
        initial: OplogEntry,
        metadata: AgentMetadata,
        status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        epoch: Option<ShardEpoch>,
    ) -> Arc<dyn Oplog> {
        self.handle(
            id,
            self.inner
                .create_fresh(
                    lifecycle, id, mode, initial, metadata, status, execution, epoch,
                )
                .await,
        )
    }

    async fn open(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        id: &OwnedAgentId,
        mode: AgentMode,
        last: Option<OplogIndex>,
        metadata: AgentMetadata,
        status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
        execution: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
        epoch: Option<ShardEpoch>,
    ) -> Arc<dyn Oplog> {
        self.handle(
            id,
            self.inner
                .open(
                    lifecycle, id, mode, last, metadata, status, execution, epoch,
                )
                .await,
        )
    }

    async fn get_last_index(&self, id: &OwnedAgentId, mode: AgentMode) -> OplogIndex {
        self.inner.get_last_index(id, mode).await
    }

    async fn try_get_last_index(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
    ) -> Result<OplogIndex, String> {
        self.inner.try_get_last_index(id, mode).await
    }

    async fn delete(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        id: &OwnedAgentId,
        mode: AgentMode,
        epoch: Option<ShardEpoch>,
    ) -> Result<(), OplogError> {
        self.inner.delete(lifecycle, id, mode, epoch).await
    }

    async fn assert_owning_epoch(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        epoch: ShardEpoch,
    ) -> Result<(), OplogError> {
        self.inner.assert_owning_epoch(id, mode, epoch).await
    }

    async fn read_exact(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        index: OplogIndex,
        count: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read_exact(id, mode, index, count).await
    }

    async fn read_source(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        index: OplogIndex,
        count: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        self.inner.read_source(id, mode, index, count).await
    }

    async fn exists(&self, id: &OwnedAgentId, mode: AgentMode) -> bool {
        self.inner.exists(id, mode).await
    }

    async fn try_exists(&self, id: &OwnedAgentId, mode: AgentMode) -> Result<bool, String> {
        self.inner.try_exists(id, mode).await
    }

    async fn scan_for_component(
        &self,
        environment: &EnvironmentId,
        component: &ComponentId,
        mode: Option<AgentMode>,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedAgentId>), WorkerExecutorError> {
        self.inner
            .scan_for_component(environment, component, mode, cursor, count)
            .await
    }

    async fn upload_raw_payload(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        data: Vec<u8>,
    ) -> Result<RawOplogPayload, String> {
        self.inner.upload_raw_payload(id, mode, data).await
    }

    async fn download_raw_payload(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        payload: PayloadId,
        hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.inner
            .download_raw_payload(id, mode, payload, hash)
            .await
    }

    async fn download_raw_payload_classified(
        &self,
        id: &OwnedAgentId,
        mode: AgentMode,
        payload: PayloadId,
        hash: Vec<u8>,
    ) -> Result<Vec<u8>, RawOplogPayloadDownloadError> {
        self.inner
            .download_raw_payload_classified(id, mode, payload, hash)
            .await
    }
}
