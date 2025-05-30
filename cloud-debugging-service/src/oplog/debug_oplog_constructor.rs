use crate::debug_session::{DebugSessionId, DebugSessions};
use crate::oplog::debug_oplog::DebugOplog;
use async_trait::async_trait;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{OwnedWorkerId, WorkerMetadata};
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
    execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
    initial_worker_metadata: WorkerMetadata,
}

impl CreateDebugOplogConstructor {
    pub fn new(
        owned_worker_id: OwnedWorkerId,
        initial_entry: Option<OplogEntry>,
        last_oplog_index: OplogIndex,
        inner: Arc<dyn OplogService + Send + Sync>,
        debug_session: Arc<dyn DebugSessions + Send + Sync>,
        execution_status: Arc<std::sync::RwLock<ExecutionStatus>>,
        initial_worker_metadata: WorkerMetadata,
    ) -> Self {
        Self {
            owned_worker_id,
            initial_entry,
            last_oplog_index,
            inner,
            debug_session,
            execution_status,
            initial_worker_metadata,
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
                    self.execution_status.clone(),
                )
                .await
        } else {
            self.inner
                .open(
                    &self.owned_worker_id,
                    self.last_oplog_index,
                    self.initial_worker_metadata.clone(),
                    self.execution_status.clone(),
                )
                .await
        };

        let debug_session_id = DebugSessionId::new(self.owned_worker_id.clone());

        Arc::new(DebugOplog::new(
            inner,
            debug_session_id,
            self.debug_session,
            self.execution_status,
            self.initial_worker_metadata,
        ))
    }
}
