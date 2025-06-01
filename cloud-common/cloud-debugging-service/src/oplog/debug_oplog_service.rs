use crate::debug_session::{DebugSessionId, DebugSessions};
use crate::oplog::debug_oplog_constructor::CreateDebugOplogConstructor;
use async_trait::async_trait;
use axum::body::Bytes;
use golem_common::model::oplog::{OplogEntry, OplogIndex, OplogPayload};
use golem_common::model::{AccountId, ComponentId, OwnedWorkerId, ScanCursor, WorkerMetadata};
use golem_worker_executor::error::GolemError;
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
        owned_worker_id: &OwnedWorkerId,
        initial_entry: OplogEntry,
        initial_worker_metadata: WorkerMetadata,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Arc<dyn Oplog + 'static> {
        self.oplogs
            .get_or_open(
                &owned_worker_id.worker_id,
                CreateDebugOplogConstructor::new(
                    owned_worker_id.clone(),
                    Some(initial_entry),
                    OplogIndex::INITIAL,
                    self.inner.clone(),
                    self.debug_session.clone(),
                    execution_status,
                    initial_worker_metadata,
                ),
            )
            .await
    }

    async fn open(
        &self,
        owned_worker_id: &OwnedWorkerId,
        last_oplog_index: OplogIndex,
        initial_worker_metadata: WorkerMetadata,
        execution_status: Arc<RwLock<ExecutionStatus>>,
    ) -> Arc<dyn Oplog + 'static> {
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
        account_id: &AccountId,
        component_id: &ComponentId,
        cursor: ScanCursor,
        count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), GolemError> {
        self.inner
            .scan_for_component(account_id, component_id, cursor, count)
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
