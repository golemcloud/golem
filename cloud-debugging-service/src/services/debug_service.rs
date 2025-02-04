use crate::auth::AuthService;
use crate::debug_context::DebugContext;
use crate::debug_session::{ActiveSession, DebugSessionData, DebugSessionId, DebugSessions};
use crate::model::params::*;
use async_trait::async_trait;
use axum_jrpc::error::{JsonRpcError, JsonRpcErrorReason};
use cloud_common::auth::CloudAuthCtx;
use cloud_common::model::ProjectAction;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AccountId, OwnedWorkerId, WorkerId, WorkerMetadata};
use golem_worker_executor_base::services::{All, HasExtraDeps, HasOplog, HasWorkerService};
use golem_worker_executor_base::worker::Worker;
use serde_json::Value;
use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::error;

#[async_trait]
pub trait DebugService {
    async fn connect(
        &self,
        authentication_context: &CloudAuthCtx,
        source_worker_id: WorkerId,
        active_session: Arc<ActiveSession>,
    ) -> Result<ConnectResult, DebugServiceError>;

    async fn playback(
        &self,
        owned_worker_id: OwnedWorkerId,
        target_index: OplogIndex,
        overrides: Option<Vec<PlaybackOverride>>,
    ) -> Result<PlaybackResult, DebugServiceError>;

    async fn rewind(
        &self,
        owned_worker_id: OwnedWorkerId,
        target_index: OplogIndex,
    ) -> Result<RewindResult, DebugServiceError>;

    async fn fork(
        &self,
        source_owned_worker_id: OwnedWorkerId,
        target_worker_id: WorkerId,
    ) -> Result<ForkResult, DebugServiceError>;

    async fn terminate_session(&self, worker_id: OwnedWorkerId) -> Result<(), DebugServiceError>;
}

#[derive(Clone, Debug)]
pub enum DebugServiceError {
    Internal {
        worker_id: Option<WorkerId>,
        message: String,
    },
    Unauthorized {
        message: String,
    },
    Conflict {
        worker_id: WorkerId,
        message: String,
    },
}

impl Display for DebugServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DebugServiceError::Internal { message, .. } => write!(f, "Internal error: {}", message),
            DebugServiceError::Unauthorized { message } => write!(f, "Unauthorized: {}", message),
            DebugServiceError::Conflict { message, .. } => write!(f, "Conflict: {}", message),
        }
    }
}

impl DebugServiceError {
    pub fn conflict(worker_id: WorkerId, message: String) -> Self {
        DebugServiceError::Conflict { worker_id, message }
    }

    pub fn unauthorized(message: String) -> Self {
        DebugServiceError::Unauthorized { message }
    }

    pub fn internal(message: String, worker_id: Option<WorkerId>) -> Self {
        DebugServiceError::Internal { worker_id, message }
    }

    pub fn get_worker_id(&self) -> Option<WorkerId> {
        match self {
            DebugServiceError::Internal { worker_id, .. } => worker_id.clone(),
            DebugServiceError::Unauthorized { .. } => None,
            DebugServiceError::Conflict { worker_id, .. } => Some(worker_id.clone()),
        }
    }

    pub fn to_rpc_error(&self) -> JsonRpcError {
        match self {
            DebugServiceError::Internal { message, .. } => JsonRpcError::new(
                JsonRpcErrorReason::InternalError,
                message.to_string(),
                Value::Null,
            ),
            DebugServiceError::Unauthorized { message } => JsonRpcError::new(
                JsonRpcErrorReason::ApplicationError(-32001),
                message.to_string(),
                Value::Null,
            ),
            DebugServiceError::Conflict { message, .. } => JsonRpcError::new(
                JsonRpcErrorReason::ApplicationError(-32002),
                message.to_string(),
                Value::Null,
            ),
        }
    }
}

pub struct DebugServiceDefault {
    worker_auth_service: Arc<dyn AuthService + Sync + Send>,
    debug_session: Arc<dyn DebugSessions + Sync + Send>,
    all: All<DebugContext>,
}

impl DebugServiceDefault {
    pub fn new(all: All<DebugContext>) -> Self {
        let extra_deps = all.extra_deps();
        let debug_session = extra_deps.debug_session();
        let worker_auth_service = extra_deps.auth_service();

        Self {
            worker_auth_service,
            debug_session,
            all,
        }
    }

    // Launches/migrate the worker to the debug mode
    // First step is to get the worker details that's currently being run
    async fn connect_worker(
        &self,
        worker_id: WorkerId,
        account_id: AccountId,
    ) -> Result<WorkerMetadata, DebugServiceError> {
        let owned_worker_id = OwnedWorkerId::new(&account_id, &worker_id);

        // This get will only look at the oplogs to see if a worker presumably exists in the real executor.
        // This is only used to get the existing metadata that was/is running in the real executor
        let existing_metadata = self.all.worker_service().get(&owned_worker_id).await;

        if let Some(existing_metadata) = existing_metadata {
            let worker_args = existing_metadata.args;
            let worker_env = existing_metadata.env;
            let component_version = existing_metadata.last_known_status.component_version;

            let parent = existing_metadata.parent;

            let worker = Worker::get_or_create_suspended(
                &self.all,
                &owned_worker_id,
                Some(worker_args),
                Some(worker_env),
                Some(component_version),
                parent,
            )
            .await
            .map_err(|e| DebugServiceError::internal(e.to_string(), Some(worker_id.clone())))?;

            let metadata = worker
                .get_metadata()
                .await
                .map_err(|e| DebugServiceError::internal(e.to_string(), Some(worker_id.clone())))?;

            Ok(metadata)
        } else {
            Err(DebugServiceError::internal(
                "Worker doesn't exist in live/real worker executor for it to connect to"
                    .to_string(),
                Some(worker_id.clone()),
            ))
        }
    }

    async fn resume_replay_with_target_index(
        &self,
        worker_id: &WorkerId,
        account_id: &AccountId,
        previous_target_oplog_index: Option<OplogIndex>,
        target_index: OplogIndex,
    ) -> Result<OplogIndex, DebugServiceError> {
        let owned_worker_id = OwnedWorkerId::new(account_id, worker_id);

        let debug_session_id = DebugSessionId::new(owned_worker_id);

        let session_data =
            self.debug_session
                .get(&debug_session_id)
                .await
                .ok_or(DebugServiceError::internal(
                    "No debug session found".to_string(),
                    Some(worker_id.clone()),
                ))?;

        if let Some(existing_target_index) = previous_target_oplog_index {
            if target_index < existing_target_index {
                return Err(DebugServiceError::internal(
                    format!(
                        "Target oplog index {} is less than the existing target oplog index {}",
                        target_index, existing_target_index
                    ),
                    Some(debug_session_id.worker_id()),
                ));
            }
        }

        if let Some(worker_metadata) = session_data.worker_metadata {
            // At this point, the worker do exist after the connect
            // however, the debug session is updated with a different target index
            // allowing replaying to (potentially) stop at this index
            let worker = Worker::get_or_create_suspended(
                &self.all,
                &OwnedWorkerId::new(account_id, worker_id),
                Some(worker_metadata.args),
                Some(worker_metadata.env),
                Some(worker_metadata.last_known_status.component_version),
                worker_metadata.parent,
            )
            .await
            .map_err(|e| DebugServiceError::internal(e.to_string(), Some(worker_id.clone())))?;

            // We select a new target index based on the given target index
            // such that it is always in an invocation boundary
            let new_target_index = Self::new_target_index(&worker, target_index).await;

            // We update the session with the new target index
            // before starting the worker
            self.debug_session
                .update(debug_session_id.clone(), new_target_index)
                .await;

            worker
                .resume_replay()
                .await
                .map_err(|e| DebugServiceError::internal(e.to_string(), Some(worker_id.clone())))?;

            Ok(new_target_index)
        } else {
            Err(DebugServiceError::internal(
                "No initial metadata found".to_string(),
                Some(worker_id.clone()),
            ))
        }
    }

    pub async fn new_target_index(
        worker: &Arc<Worker<DebugContext>>,
        target_oplog_index: OplogIndex,
    ) -> OplogIndex {
        // New target index to be calculated here
        let oplog = worker.oplog();

        let original_current_oplog_index = oplog.current_oplog_index().await;

        if target_oplog_index < original_current_oplog_index {
            Self::get_target_oplog_index_at_invocation_boundary(
                |index| {
                    let inner = oplog.clone();
                    Box::pin(async move { inner.read(index).await })
                },
                target_oplog_index,
                original_current_oplog_index,
            )
            .await
            .expect("Internal Error. Invocation boundary not found")
        } else {
            original_current_oplog_index
        }
    }

    pub async fn get_target_oplog_index_at_invocation_boundary<F>(
        read_oplog_entry: F,
        target_oplog_index: OplogIndex,
        original_last_oplog_index: OplogIndex,
    ) -> Result<OplogIndex, String>
    where
        F: Fn(OplogIndex) -> Pin<Box<dyn Future<Output = OplogEntry> + Send>>,
    {
        let mut new_target_oplog_index = target_oplog_index;

        loop {
            let entry = read_oplog_entry(new_target_oplog_index).await;

            match entry {
                OplogEntry::ExportedFunctionCompleted { .. } => {
                    return Ok(new_target_oplog_index);
                }
                _ => {
                    if new_target_oplog_index == original_last_oplog_index {
                        let error_message = format!(
                            "Invocation boundary not found. Set an oplog index that is not in the middle of an incomplete invocation. \
                        Last oplog index: {}",
                            original_last_oplog_index
                        );
                        error!("{}", error_message);
                        return Err(error_message);
                    }

                    new_target_oplog_index = new_target_oplog_index.next();
                }
            }
        }
    }
}

#[async_trait]
impl DebugService for DebugServiceDefault {
    async fn connect(
        &self,
        auth_ctx: &CloudAuthCtx,
        worker_id: WorkerId,
        active_session: Arc<ActiveSession>,
    ) -> Result<ConnectResult, DebugServiceError> {
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(
                &worker_id.component_id,
                ProjectAction::UpdateWorker,
                auth_ctx,
            )
            .await
            .map_err(|e| DebugServiceError::unauthorized(format!("Unauthorized: {}", e)))?;

        let owned_worker_id = OwnedWorkerId::new(&namespace.account_id, &worker_id);

        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        if self.debug_session.get(&debug_session_id).await.is_some() {
            return Err(DebugServiceError::conflict(
                worker_id.clone(),
                "Worker is already being debugged".to_string(),
            ));
        }

        // Ensuring active session is set with the fundamental details of worker_id that is going to be debugged
        active_session
            .set_active_session(owned_worker_id.worker_id.clone(), namespace.clone())
            .await;

        // This simply migrates the worker to the debug mode, but it doesn't start the worker
        let metadata = self
            .connect_worker(worker_id.clone(), namespace.account_id.clone())
            .await?;

        self.debug_session
            .insert(
                DebugSessionId::new(owned_worker_id),
                DebugSessionData {
                    worker_metadata: Some(metadata),
                    target_oplog_index_at_invocation_boundary: None,
                },
            )
            .await;

        Ok(ConnectResult {
            worker_id: worker_id.clone(),
            success: true,
            message: format!("Worker {} connected to namespace {}", worker_id, namespace),
        })
    }

    async fn playback(
        &self,
        owned_worker_id: OwnedWorkerId,
        target_index: OplogIndex,
        _overrides: Option<Vec<PlaybackOverride>>,
    ) -> Result<PlaybackResult, DebugServiceError> {
        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        let existing_session_data =
            self.debug_session
                .get(&debug_session_id)
                .await
                .ok_or(DebugServiceError::internal(
                    "No debug session found".to_string(),
                    Some(owned_worker_id.worker_id.clone()),
                ))?;

        let existing_target_index = existing_session_data.target_oplog_index_at_invocation_boundary;

        let stopped_at_index = self
            .resume_replay_with_target_index(
                &owned_worker_id.worker_id,
                &owned_worker_id.account_id,
                existing_target_index,
                target_index,
            )
            .await?;

        Ok(PlaybackResult {
            worker_id: owned_worker_id.worker_id.clone(),
            stopped_at_index,
            success: true,
            message: format!(
                "Playback worker {} stopped at index {}",
                owned_worker_id.worker_id, target_index
            ),
        })
    }

    async fn rewind(
        &self,
        owned_worker_id: OwnedWorkerId,
        target_oplog_index: OplogIndex,
    ) -> Result<RewindResult, DebugServiceError> {
        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        self.debug_session
            .update(debug_session_id, target_oplog_index)
            .await;

        Ok(RewindResult {
            worker_id: owned_worker_id.worker_id,
            success: true,
            message: format!("Rewinding the worker to index {}", target_oplog_index),
        })
    }

    async fn fork(
        &self,
        source_worker_id: OwnedWorkerId,
        target_worker_id: WorkerId,
    ) -> Result<ForkResult, DebugServiceError> {
        Ok(ForkResult {
            source_worker_id: source_worker_id.worker_id.clone(),
            target_worker_id: target_worker_id.clone(),
            success: true,
            message: format!(
                "Forked worker {} to new worker {}",
                source_worker_id, target_worker_id
            ),
        })
    }

    async fn terminate_session(
        &self,
        owned_worker_id: OwnedWorkerId,
    ) -> Result<(), DebugServiceError> {
        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        self.debug_session
            .remove(debug_session_id)
            .await
            .ok_or(DebugServiceError::internal(
                "No debug session found".to_string(),
                Some(owned_worker_id.worker_id),
            ))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Bytes;
    use test_r::test;

    use super::*;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::oplog::{OplogEntry, OplogPayload};
    use golem_common::model::Timestamp;

    #[test]
    async fn test_get_target_oplog_index_at_invocation_boundary_1() {
        let target_oplog_index = OplogIndex::from_u64(1);
        let original_last_oplog_index = OplogIndex::from_u64(10);

        let result = DebugServiceDefault::get_target_oplog_index_at_invocation_boundary(
            read_oplog(5),
            target_oplog_index,
            original_last_oplog_index,
        )
        .await;

        assert_eq!(result, Ok(OplogIndex::from_u64(5)));
    }

    #[test]
    async fn test_get_target_oplog_index_at_invocation_boundary_2() {
        let target_oplog_index = OplogIndex::from_u64(1);
        let original_last_oplog_index = OplogIndex::from_u64(10);

        let result = DebugServiceDefault::get_target_oplog_index_at_invocation_boundary(
            read_oplog(11),
            target_oplog_index,
            original_last_oplog_index,
        )
        .await;

        assert!(result.is_err());
    }

    type OplogEntryFuture = Pin<Box<dyn Future<Output = OplogEntry> + Send>>;

    fn read_oplog(invocation_completion_index: u64) -> Box<dyn Fn(OplogIndex) -> OplogEntryFuture> {
        Box::new(move |index: OplogIndex| {
            Box::pin(async move {
                if index == OplogIndex::from_u64(invocation_completion_index) {
                    OplogEntry::ExportedFunctionCompleted {
                        timestamp: Timestamp::now_utc(),
                        response: OplogPayload::Inline(Bytes::new().into()),
                        consumed_fuel: 0,
                    }
                } else {
                    // Any other oplog entry other than export function completed
                    OplogEntry::NoOp {
                        timestamp: Timestamp::now_utc(),
                    }
                }
            })
        })
    }
}
