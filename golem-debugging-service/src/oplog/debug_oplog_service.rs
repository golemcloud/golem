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

use crate::debug_session::{DebugSessionId, DebugSessions};
use crate::oplog::debug_oplog_constructor::CreateDebugOplogConstructor;
use async_trait::async_trait;
use bytes::Bytes;
use golem_common::base_model::ProjectId;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::{ComponentId, OwnedWorkerId, ScanCursor, WorkerMetadata};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_worker_executor::model::ExecutionStatus;
use golem_worker_executor::services::oplog::{OpenOplogs, Oplog, OplogService};
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, RwLock};

pub struct DebugOplogService {
    pub inner: Arc<dyn OplogService>,
    oplogs: OpenOplogs,
    pub debug_session: Arc<dyn DebugSessions>,
}

impl DebugOplogService {
    pub fn new(inner: Arc<dyn OplogService>, debug_session: Arc<dyn DebugSessions>) -> Self {
        Self {
            inner,
            debug_session,
            oplogs: OpenOplogs::new("debugging_oplog_service"),
        }
    }
}

impl Debug for DebugOplogService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugOplogService").finish()
    }
}

#[async_trait]
impl OplogService for DebugOplogService {
    async fn create(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        _initial_entry: OplogEntry,
        _initial_worker_metadata: WorkerMetadata,
        _execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Arc<dyn Oplog> {
        panic!("Cannot create a new oplog when debugging")
    }

    async fn open(
        &self,
        owned_worker_id: &OwnedWorkerId,
        last_oplog_index: OplogIndex,
        initial_worker_metadata: WorkerMetadata,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Arc<dyn Oplog> {
        self.oplogs
            .get_or_open(
                &owned_worker_id.worker_id,
                CreateDebugOplogConstructor::new(
                    owned_worker_id.clone(),
                    None,
                    last_oplog_index,
                    self.inner.clone(),
                    self.debug_session.clone(),
                    execution_status,
                    initial_worker_metadata,
                ),
            )
            .await
    }

    async fn get_last_index(&self, owned_worker_id: &OwnedWorkerId) -> OplogIndex {
        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        let result = self
            .debug_session
            .get(&debug_session_id)
            .await
            .and_then(|debug_session| debug_session.target_oplog_index);

        match result {
            Some(index) => index,
            None => self.inner.get_last_index(owned_worker_id).await,
        }
    }

    async fn delete(&self, owned_worker_id: &OwnedWorkerId) {
        self.inner.delete(owned_worker_id).await
    }

    async fn read(
        &self,
        owned_worker_id: &OwnedWorkerId,
        idx: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        // In a debugging service, the read happens only through resume_replay which implies every call to
        // oplog_service.read will be always part of a replay (and never live)
        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());
        self.debug_session
            .update_oplog_index(debug_session_id, idx)
            .await;
        self.inner.read(owned_worker_id, idx, n).await
    }

    async fn exists(&self, owned_worker_id: &OwnedWorkerId) -> bool {
        self.inner.exists(owned_worker_id).await
    }

    async fn scan_for_component(
        &self,
        project_id: &ProjectId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), WorkerExecutorError> {
        self.inner
            .scan_for_component(project_id, component_id, cursor, count)
            .await
    }

    // DebugService shouldn't upload any data to the oplog
    async fn upload_payload(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        data: &[u8],
    ) -> Result<OplogPayload, String> {
        Ok(OplogPayload::Inline(data.to_vec()))
    }

    async fn download_payload(
        &self,
        owned_worker_id: &OwnedWorkerId,
        payload: &OplogPayload,
    ) -> Result<Bytes, String> {
        self.inner.download_payload(owned_worker_id, payload).await
    }
}
