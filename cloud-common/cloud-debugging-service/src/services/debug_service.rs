use crate::auth::AuthService;
use crate::debug_context::DebugContext;
use crate::debug_session::{ActiveSession, PlaybackOverridesInternal};
use crate::debug_session::{DebugSessionData, DebugSessionId, DebugSessions};
use crate::model::params::*;
use async_trait::async_trait;
use axum_jrpc::error::{JsonRpcError, JsonRpcErrorReason};
use cloud_common::auth::CloudAuthCtx;
use cloud_common::model::ProjectAction;
use gethostname::gethostname;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AccountId, OwnedWorkerId, WorkerId, WorkerMetadata};
use golem_worker_executor::model::InterruptKind;
use golem_worker_executor::services::oplog::Oplog;
use golem_worker_executor::services::{
    All, HasConfig, HasExtraDeps, HasOplog, HasShardManagerService, HasShardService,
    HasWorkerForkService, HasWorkerService,
};
use golem_worker_executor::worker::Worker;
use golem_worker_executor::GolemTypes;
use serde_json::Value;
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

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
        ensure_invocation_boundary: bool,
        wait_time: Duration,
    ) -> Result<PlaybackResult, DebugServiceError>;

    async fn rewind(
        &self,
        owned_worker_id: OwnedWorkerId,
        target_index: OplogIndex,
        ensure_invocation_boundary: bool,
        timeout: Duration,
    ) -> Result<RewindResult, DebugServiceError>;

    async fn fork(
        &self,
        source_owned_worker_id: OwnedWorkerId,
        target_worker_id: WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<ForkResult, DebugServiceError>;

    async fn current_oplog_index(
        &self,
        worker_id: OwnedWorkerId,
    ) -> Result<OplogIndex, DebugServiceError>;

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

    ValidationFailed {
        worker_id: Option<WorkerId>,
        errors: Vec<String>,
    },
}

impl Display for DebugServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DebugServiceError::Internal { message, .. } => write!(f, "Internal error: {}", message),
            DebugServiceError::Unauthorized { message } => write!(f, "Unauthorized: {}", message),
            DebugServiceError::Conflict { message, .. } => write!(f, "Conflict: {}", message),
            DebugServiceError::ValidationFailed { errors, .. } => {
                write!(f, "Validation failed: {:?}", errors.join(", "))
            }
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

    pub fn validation_failed(errors: Vec<String>, worker_id: Option<WorkerId>) -> Self {
        DebugServiceError::ValidationFailed { errors, worker_id }
    }

    pub fn get_worker_id(&self) -> Option<WorkerId> {
        match self {
            DebugServiceError::Internal { worker_id, .. } => (*worker_id).clone(),
            DebugServiceError::Unauthorized { .. } => None,
            DebugServiceError::Conflict { worker_id, .. } => Some(worker_id.clone()),
            DebugServiceError::ValidationFailed { worker_id, .. } => (*worker_id).clone(),
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
            DebugServiceError::ValidationFailed { errors, .. } => JsonRpcError::new(
                JsonRpcErrorReason::ApplicationError(-32004),
                errors.join(", "),
                Value::Null,
            ),
        }
    }
}

pub struct DebugServiceDefault<T: GolemTypes> {
    worker_auth_service: Arc<dyn AuthService + Sync + Send>,
    debug_session: Arc<dyn DebugSessions + Sync + Send>,
    all: All<DebugContext<T>>,
}

impl<T: GolemTypes> DebugServiceDefault<T> {
    pub fn new(all: All<DebugContext<T>>) -> Self {
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

        let host = gethostname().to_string_lossy().to_string();

        let port = self.all.config().port;

        info!(
            "Registering worker {} with host {} and port {}",
            worker_id, host, port
        );

        let shard_assignment = self
            .all
            .shard_manager_service()
            .register(host, port)
            .await
            .map_err(|e| DebugServiceError::internal(e.to_string(), Some(worker_id.clone())))?;

        self.all.shard_service().register(
            shard_assignment.number_of_shards,
            &shard_assignment.shard_ids,
        );

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

    async fn rewind(
        &self,
        owned_worker_id: &OwnedWorkerId,
        target_index: &OplogIndex,
        ensure_invocation_boundary: bool,
        wait_time: Duration,
    ) -> Result<RewindResult, DebugServiceError> {
        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        let debug_session_data =
            self.debug_session
                .get(&debug_session_id)
                .await
                .ok_or(DebugServiceError::internal(
                    "No debug session found. Rewind can be called ".to_string(),
                    Some(owned_worker_id.worker_id.clone()),
                ))?;

        if let Some(current_oplog_index) = debug_session_data.target_oplog_index {
            let worker = Worker::get_or_create_suspended(
                &self.all,
                owned_worker_id,
                debug_session_data
                    .worker_metadata
                    .as_ref()
                    .map(|m| m.args.clone()),
                debug_session_data
                    .worker_metadata
                    .as_ref()
                    .map(|m| m.env.clone()),
                debug_session_data
                    .worker_metadata
                    .as_ref()
                    .map(|m| m.last_known_status.component_version),
                debug_session_data
                    .worker_metadata
                    .as_ref()
                    .and_then(|m| m.parent.clone()),
            )
            .await
            .map_err(|e| {
                DebugServiceError::internal(e.to_string(), Some(owned_worker_id.worker_id.clone()))
            })?;

            let new_target_index = if ensure_invocation_boundary {
                Self::get_target_oplog_index_at_invocation_boundary(
                    worker.oplog(),
                    *target_index,
                    current_oplog_index,
                )
                .await
                .map_err(|e| {
                    DebugServiceError::internal(e, Some(owned_worker_id.worker_id.clone()))
                })?
            } else {
                *target_index
            };

            if new_target_index > current_oplog_index {
                return Err(DebugServiceError::validation_failed(
                    vec![
                        format!(
                            "Target oplog index {} (corresponding to an invocation boundary) for rewind is greater than the existing target oplog index {}",
                            target_index,
                            current_oplog_index
                        )],
                        Some(owned_worker_id.worker_id.clone()))
                 );
            };

            self.debug_session
                .update(debug_session_id.clone(), new_target_index, None)
                .await;

            self.debug_session
                .update_oplog_index(debug_session_id.clone(), OplogIndex::NONE)
                .await;

            // we restart regardless of the current status of the worker such that it restarts
            worker.set_interrupting(InterruptKind::Restart).await;

            tokio::time::sleep(wait_time).await;

            let last_index = self
                .debug_session
                .get(&debug_session_id)
                .await
                .map(|d| d.current_oplog_index)
                .unwrap_or(OplogIndex::NONE);

            Ok(RewindResult {
                worker_id: owned_worker_id.worker_id.clone(),
                current_index: last_index,
                success: true,
                message: format!("Rewinding the worker to index {}", target_index),
            })
        } else {
            // If this is the first step in a debugging session, then rewind is more or less
            // playback to that index
            self.playback(
                owned_worker_id.clone(),
                *target_index,
                None,
                ensure_invocation_boundary,
                wait_time,
            )
            .await
            .map(|result| RewindResult {
                worker_id: owned_worker_id.worker_id.clone(),
                current_index: result.current_index,
                success: true,
                message: format!("Rewinding the worker to index {}", target_index),
            })
        }
    }

    async fn resume_replay_with_target_index(
        &self,
        worker_id: &WorkerId,
        account_id: &AccountId,
        existing_target_oplog_index: Option<OplogIndex>,
        target_index: OplogIndex,
        playback_overrides: Option<Vec<PlaybackOverride>>,
        ensure_invocation_boundary: bool,
        timeout: Duration,
    ) -> Result<OplogIndex, DebugServiceError> {
        let owned_worker_id = OwnedWorkerId::new(account_id, worker_id);

        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        let session_data =
            self.debug_session
                .get(&debug_session_id)
                .await
                .ok_or(DebugServiceError::internal(
                    "No debug session found".to_string(),
                    Some(worker_id.clone()),
                ))?;

        if let Some(existing_target_index) = existing_target_oplog_index {
            if target_index < existing_target_index {
                return Err(DebugServiceError::internal(
                    format!(
                        "Target oplog index {} for playback is less than the existing target oplog index {}. Use rewind instead",
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
                Some(worker_metadata.args.clone()),
                Some(worker_metadata.env.clone()),
                Some(worker_metadata.last_known_status.component_version),
                worker_metadata.parent.clone(),
            )
            .await
            .map_err(|e| DebugServiceError::internal(e.to_string(), Some(worker_id.clone())))?;

            // We select a new target index based on the given target index
            // such that it is always in an invocation boundary
            let new_target_index = if ensure_invocation_boundary {
                Self::target_index_at_invocation_boundary(worker_id, &worker, target_index).await?
            } else {
                target_index
            };

            let mut playback_overrides_validated = None;

            if let Some(overrides) = playback_overrides {
                playback_overrides_validated =
                    Some(Self::validate_playback_overrides(worker_id.clone(), overrides).await?);
            }

            // We update the session with the new target index
            // before starting the worker
            self.debug_session
                .update(
                    debug_session_id.clone(),
                    new_target_index,
                    playback_overrides_validated,
                )
                .await;

            if existing_target_oplog_index.is_some() {
                worker.stop().await;
            }

            Worker::start_if_needed(worker.clone())
                .await
                .map_err(|e| DebugServiceError::internal(e.to_string(), Some(worker_id.clone())))?;

            tokio::time::sleep(timeout).await;

            let last_index = self
                .debug_session
                .get(&debug_session_id)
                .await
                .map(|d| d.current_oplog_index)
                .unwrap_or(OplogIndex::INITIAL);

            Ok(last_index)
        } else {
            Err(DebugServiceError::internal(
                "No initial metadata found".to_string(),
                Some(worker_id.clone()),
            ))
        }
    }

    pub async fn validate_playback_overrides(
        worker_id: WorkerId,
        overrides: Vec<PlaybackOverride>,
    ) -> Result<PlaybackOverridesInternal, DebugServiceError> {
        PlaybackOverridesInternal::from_playback_override(overrides).map_err(|err| {
            DebugServiceError::ValidationFailed {
                worker_id: Some(worker_id.clone()),
                errors: vec![err],
            }
        })
    }

    pub async fn target_index_at_invocation_boundary(
        worker_id: &WorkerId,
        worker: &Arc<Worker<DebugContext<T>>>,
        target_oplog_index: OplogIndex,
    ) -> Result<OplogIndex, DebugServiceError> {
        // New target index to be calculated here
        let oplog: Arc<dyn Oplog + Send + Sync> = worker.oplog();

        let original_current_oplog_index = oplog.current_oplog_index().await;

        Self::get_target_oplog_index_at_invocation_boundary(
            oplog,
            target_oplog_index,
            original_current_oplog_index,
        )
        .await
        .map_err(|e| DebugServiceError::internal(e, Some(worker_id.clone())))
    }

    pub async fn get_target_oplog_index_at_invocation_boundary(
        oplog: Arc<dyn Oplog + Send + Sync>,
        target_oplog_index: OplogIndex,
        original_last_oplog_index: OplogIndex,
    ) -> Result<OplogIndex, String> {
        let mut new_target_oplog_index = target_oplog_index;

        loop {
            let entry = oplog.read(new_target_oplog_index).await;

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
impl<T: GolemTypes> DebugService for DebugServiceDefault<T> {
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
                    target_oplog_index: None,
                    playback_overrides: PlaybackOverridesInternal::empty(),
                    current_oplog_index: OplogIndex::NONE,
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
        overrides: Option<Vec<PlaybackOverride>>,
        ensure_invocation_boundary: bool,
        timeout: Duration,
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

        let existing_target_index = existing_session_data.target_oplog_index;

        let stopped_at_index = self
            .resume_replay_with_target_index(
                &owned_worker_id.worker_id,
                &owned_worker_id.account_id,
                existing_target_index,
                target_index,
                overrides,
                ensure_invocation_boundary,
                timeout,
            )
            .await?;

        Ok(PlaybackResult {
            worker_id: owned_worker_id.worker_id.clone(),
            current_index: stopped_at_index,
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
        ensure_invocation_boundary: bool,
        timeout: Duration,
    ) -> Result<RewindResult, DebugServiceError> {
        info!(
            "Rewinding worker {} to index {}",
            owned_worker_id.worker_id, target_oplog_index
        );

        self.rewind(
            &owned_worker_id,
            &target_oplog_index,
            ensure_invocation_boundary,
            timeout,
        )
        .await
    }

    async fn fork(
        &self,
        source_worker_id: OwnedWorkerId,
        target_worker_id: WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<ForkResult, DebugServiceError> {
        info!(
            "Forking worker {} to new worker {}",
            source_worker_id.worker_id, target_worker_id
        );

        // Fork internally proxies the resume of worker using worker-proxy
        // making sure the worker is initiated in the regular worker executor, and not
        // debugging executor
        self.all
            .worker_fork_service()
            .fork(&source_worker_id, &target_worker_id, oplog_index_cut_off)
            .await
            .map_err(|e| {
                DebugServiceError::internal(e.to_string(), Some(source_worker_id.worker_id.clone()))
            })?;

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

    async fn current_oplog_index(
        &self,
        worker_id: OwnedWorkerId,
    ) -> Result<OplogIndex, DebugServiceError> {
        let debug_session_id = DebugSessionId::new(worker_id.clone());

        let result = self
            .debug_session
            .get(&debug_session_id)
            .await
            .map(|debug_session| debug_session.current_oplog_index);

        match result {
            Some(index) => Ok(index),
            None => Err(DebugServiceError::internal(
                "No debug session found".to_string(),
                Some(worker_id.worker_id),
            )),
        }
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
    use golem_worker_executor::DefaultGolemTypes;
    use std::fmt::{Debug, Formatter};
    use std::time::Duration;
    use test_r::test;

    use super::*;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::oplog::{OplogEntry, OplogPayload};
    use golem_common::model::Timestamp;
    use golem_worker_executor::services::oplog::CommitLevel;

    #[test]
    async fn test_get_target_oplog_index_at_invocation_boundary_1() {
        let target_oplog_index = OplogIndex::from_u64(1);
        let original_last_oplog_index = OplogIndex::from_u64(10);

        let result = DebugServiceDefault::<DefaultGolemTypes>::get_target_oplog_index_at_invocation_boundary(
            Arc::new(TestOplog::new(5)),
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

        let result = DebugServiceDefault::<DefaultGolemTypes>::get_target_oplog_index_at_invocation_boundary(
            Arc::new(TestOplog::new(11)),
            target_oplog_index,
            original_last_oplog_index,
        )
        .await;

        assert!(result.is_err());
    }

    struct TestOplog {
        invocation_completion_index: u64,
    }

    impl TestOplog {
        fn new(invocation_completion_index: u64) -> Self {
            Self {
                invocation_completion_index,
            }
        }
    }

    impl Debug for TestOplog {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "TestOplog")
        }
    }

    #[async_trait]
    impl Oplog for TestOplog {
        async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
            if oplog_index == OplogIndex::from_u64(self.invocation_completion_index) {
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
        }

        async fn add(&self, _entry: OplogEntry) {
            unimplemented!()
        }

        async fn drop_prefix(&self, _last_dropped_id: OplogIndex) {
            unimplemented!()
        }

        async fn commit(&self, _level: CommitLevel) {
            unimplemented!()
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            unimplemented!()
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            unimplemented!()
        }

        async fn length(&self) -> u64 {
            unimplemented!()
        }

        async fn upload_payload(&self, _data: &[u8]) -> Result<OplogPayload, String> {
            unimplemented!()
        }

        async fn download_payload(&self, _payload: &OplogPayload) -> Result<Bytes, String> {
            unimplemented!()
        }
    }
}
