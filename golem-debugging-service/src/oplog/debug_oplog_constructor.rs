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
use crate::oplog::debug_oplog::DebugOplog;
use async_trait::async_trait;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{OwnedWorkerId, WorkerMetadata, WorkerStatusRecord};
use golem_common::read_only_lock;
use golem_worker_executor::model::ExecutionStatus;
use golem_worker_executor::services::oplog::{Oplog, OplogConstructor, OplogService};
use std::sync::Arc;

#[derive(Clone)]
pub struct CreateDebugOplogConstructor {
    owned_worker_id: OwnedWorkerId,
    initial_entry: Option<OplogEntry>,
    last_oplog_index: OplogIndex,
    inner: Arc<dyn OplogService + Send + Sync>,
    debug_session: Arc<dyn DebugSessions + Send + Sync>,
    initial_worker_metadata: WorkerMetadata,
    last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
    execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
}

impl CreateDebugOplogConstructor {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        initial_entry: Option<OplogEntry>,
        last_oplog_index: OplogIndex,
        inner: Arc<dyn OplogService + Send + Sync>,
        debug_session: Arc<dyn DebugSessions + Send + Sync>,
        initial_worker_metadata: WorkerMetadata,
        last_known_status: read_only_lock::tokio::ReadOnlyLock<WorkerStatusRecord>,
        execution_status: read_only_lock::std::ReadOnlyLock<ExecutionStatus>,
    ) -> Self {
        Self {
            owned_worker_id,
            initial_entry,
            last_oplog_index,
            inner,
            debug_session,
            initial_worker_metadata,
            last_known_status,
            execution_status,
        }
    }
}

#[async_trait]
impl OplogConstructor for CreateDebugOplogConstructor {
    async fn create_oplog(self, _close: Box<dyn FnOnce() + Send + Sync>) -> Arc<dyn Oplog> {
        let inner = if let Some(initial_entry) = self.initial_entry {
            self.inner
                .create(
                    &self.owned_worker_id,
                    initial_entry,
                    self.initial_worker_metadata.clone(),
                    self.last_known_status.clone(),
                    self.execution_status.clone(),
                )
                .await
        } else {
            self.inner
                .open(
                    &self.owned_worker_id,
                    self.last_oplog_index,
                    self.initial_worker_metadata.clone(),
                    self.last_known_status.clone(),
                    self.execution_status.clone(),
                )
                .await
        };

        let debug_session_id = DebugSessionId::new(self.owned_worker_id.clone());

        Arc::new(DebugOplog::new(inner, debug_session_id, self.debug_session))
    }
}
