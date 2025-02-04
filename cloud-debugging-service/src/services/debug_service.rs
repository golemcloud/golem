use crate::auth::AuthService;
use crate::debug_session::{ActiveSession, DebugSession, DebugSessionData, DebugSessionId};
use crate::model::params::*;
use async_trait::async_trait;
use axum_jrpc::error::{JsonRpcError, JsonRpcErrorReason};
use cloud_common::auth::CloudAuthCtx;
use cloud_common::model::ProjectAction;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{OwnedWorkerId, WorkerId};
use serde_json::Value;
use std::fmt::Display;
use std::sync::Arc;

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
    debug_session: Arc<dyn DebugSession + Sync + Send>,
}

impl DebugServiceDefault {
    pub fn new(
        worker_auth_service: Arc<dyn AuthService + Sync + Send>,
        debug_session: Arc<dyn DebugSession + Sync + Send>,
    ) -> Self {
        Self {
            worker_auth_service,
            debug_session,
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

        let currently_debugged = self.debug_session.get(&debug_session_id).await;

        if currently_debugged.is_some() {
            return Err(DebugServiceError::conflict(
                worker_id.clone(),
                "Worker is already being debugged".to_string(),
            ));
        }

        // Ensuring active session is set with the fundamental details of worker_id that is going to be debugged
        active_session
            .set_active_session(owned_worker_id.worker_id.clone(), namespace.clone())
            .await;

        // Ensuring a shared debug_session is set
        self.debug_session
            .insert(
                DebugSessionId::new(owned_worker_id),
                DebugSessionData {
                    target_oplog_index: None,
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

        self.debug_session
            .insert(
                debug_session_id.clone(),
                DebugSessionData {
                    target_oplog_index: Some(target_index),
                },
            )
            .await;

        Ok(PlaybackResult {
            worker_id: owned_worker_id.worker_id.clone(),
            stopped_at_index: target_index,
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
        target_index: OplogIndex,
    ) -> Result<RewindResult, DebugServiceError> {
        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        self.debug_session
            .insert(
                debug_session_id.clone(),
                DebugSessionData {
                    target_oplog_index: Some(target_index),
                },
            )
            .await;

        Ok(RewindResult {
            worker_id: owned_worker_id.worker_id.clone(),
            success: true,
            message: format!("Rewinding the worker to index {}", target_index),
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
