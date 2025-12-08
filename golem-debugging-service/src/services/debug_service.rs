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

use super::auth::{AuthService, AuthServiceError};
use crate::debug_context::DebugContext;
use crate::debug_session::PlaybackOverridesInternal;
use crate::debug_session::{DebugSessionData, DebugSessionId, DebugSessions};
use crate::model::params::*;
use async_trait::async_trait;
use gethostname::gethostname;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{OwnedWorkerId, WorkerId, WorkerMetadata};
use golem_common::SafeDisplay;
use golem_service_base::error::worker_executor::InterruptKind;
use golem_service_base::model::auth::AuthCtx;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::services::oplog::Oplog;
use golem_worker_executor::services::worker_event::WorkerEventReceiver;
use golem_worker_executor::services::{
    All, HasComponentService, HasConfig, HasExtraDeps, HasOplog, HasShardManagerService,
    HasShardService, HasWorkerForkService, HasWorkerService,
};
use golem_worker_executor::worker::Worker;
use log::debug;
use std::fmt::Display;
use std::sync::Arc;
use tracing::{error, info};

#[async_trait]
pub trait DebugService: Send + Sync {
    async fn connect(
        &self,
        authentication_context: &AuthCtx,
        source_worker_id: &WorkerId,
    ) -> Result<(ConnectResult, AccountId, OwnedWorkerId, WorkerEventReceiver), DebugServiceError>;

    async fn playback(
        &self,
        owned_worker_id: &OwnedWorkerId,
        account_id: &AccountId,
        target_index: OplogIndex,
        overrides: Option<Vec<PlaybackOverride>>,
        ensure_invocation_boundary: bool,
    ) -> Result<PlaybackResult, DebugServiceError>;

    async fn rewind(
        &self,
        owned_worker_id: &OwnedWorkerId,
        account_id: &AccountId,
        target_index: OplogIndex,
        ensure_invocation_boundary: bool,
    ) -> Result<RewindResult, DebugServiceError>;

    async fn fork(
        &self,
        account_id: &AccountId,
        source_owned_worker_id: &OwnedWorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<ForkResult, DebugServiceError>;

    async fn current_oplog_index(
        &self,
        worker_id: &OwnedWorkerId,
    ) -> Result<OplogIndex, DebugServiceError>;

    async fn terminate_session(&self, worker_id: &OwnedWorkerId) -> Result<(), DebugServiceError>;
}

#[derive(Clone, Debug)]
pub enum DebugServiceError {
    Internal {
        worker_id: Option<WorkerId>,
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

impl DebugServiceError {
    pub fn conflict(worker_id: WorkerId, message: String) -> Self {
        DebugServiceError::Conflict { worker_id, message }
    }

    pub fn unauthorized(message: String) -> Self {
        DebugServiceError::Unauthorized { message }
    }

    pub fn internal(message: String, worker_id: Option<WorkerId>) -> Self {
        tracing::warn!("internal error in debugging service: {message}");
        DebugServiceError::Internal { worker_id }
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
}

impl SafeDisplay for DebugServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            DebugServiceError::Internal { .. } => "Internal error".to_string(),
            DebugServiceError::Unauthorized { .. } => self.to_string(),
            DebugServiceError::Conflict { .. } => self.to_string(),
            DebugServiceError::ValidationFailed { .. } => self.to_string(),
        }
    }
}

impl Display for DebugServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DebugServiceError::Internal { .. } => write!(f, "Internal error"),
            DebugServiceError::Unauthorized { message } => write!(f, "Unauthorized: {message}"),
            DebugServiceError::Conflict { message, .. } => write!(f, "Conflict: {message}"),
            DebugServiceError::ValidationFailed { errors, .. } => {
                write!(f, "Validation failed: {:?}", errors.join(", "))
            }
        }
    }
}

pub struct DebugServiceDefault {
    component_service: Arc<dyn ComponentService>,
    debug_session: Arc<dyn DebugSessions>,
    auth_service: Arc<dyn AuthService>,
    all: All<DebugContext>,
}

impl DebugServiceDefault {
    pub fn new(all: All<DebugContext>) -> Self {
        let component_service = all.component_service();
        let extra_deps = all.extra_deps();
        let debug_session = extra_deps.debug_session();
        let auth_service = extra_deps.auth_service();

        Self {
            component_service,
            debug_session,
            auth_service,
            all,
        }
    }

    // Launches/migrate the worker to the debug mode
    // First step is to get the worker details that's currently being run
    async fn connect_worker(
        &self,
        worker_id: WorkerId,
        account_id: &AccountId,
        enironment_id: &EnvironmentId,
    ) -> Result<(WorkerMetadata, WorkerEventReceiver), DebugServiceError> {
        let owned_worker_id = OwnedWorkerId::new(enironment_id, &worker_id);

        // This get will only look at the oplogs to see if a worker presumably exists in the real executor.
        // This is only used to get the existing metadata that was/is running in the real executor
        self.all
            .worker_service()
            .get(&owned_worker_id)
            .await
            .ok_or_else(|| {
                DebugServiceError::conflict(
                    worker_id.clone(),
                    "Worker doesn't exist in live/real worker executor for it to connect to"
                        .to_string(),
                )
            })?;

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

        let worker = Worker::get_or_create_suspended(
            &self.all,
            account_id,
            &owned_worker_id,
            None,
            None,
            None,
            None,
            &InvocationContextStack::fresh(),
        )
        .await
        .map_err(|e| DebugServiceError::internal(e.to_string(), Some(worker_id.clone())))?;

        let metadata = worker.get_latest_worker_metadata().await;

        let receiver = worker.event_service().receiver();

        Ok((metadata, receiver))
    }

    pub async fn validate_playback_overrides(
        worker_id: WorkerId,
        current_index: OplogIndex,
        overrides: Vec<PlaybackOverride>,
    ) -> Result<PlaybackOverridesInternal, DebugServiceError> {
        PlaybackOverridesInternal::from_playback_override(overrides, current_index).map_err(|err| {
            DebugServiceError::ValidationFailed {
                worker_id: Some(worker_id.clone()),
                errors: vec![err],
            }
        })
    }

    pub async fn target_index_at_invocation_boundary(
        worker_id: &WorkerId,
        worker: &Arc<Worker<DebugContext>>,
        target_oplog_index: OplogIndex,
    ) -> Result<OplogIndex, DebugServiceError> {
        // New target index to be calculated here
        let oplog: Arc<dyn Oplog> = worker.oplog();

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
        oplog: Arc<dyn Oplog>,
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
                        Last oplog index: {original_last_oplog_index}"
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
        auth_ctx: &AuthCtx,
        worker_id: &WorkerId,
    ) -> Result<(ConnectResult, AccountId, OwnedWorkerId, WorkerEventReceiver), DebugServiceError>
    {
        let component = self
            .component_service
            .get_metadata(&worker_id.component_id, None)
            .await
            .map_err(|e| DebugServiceError::internal(e.to_string(), Some(worker_id.clone())))?;

        self.auth_service
            .check_user_allowed_to_debug_in_environment(&component.environment_id, auth_ctx)
            .await
            .map_err(|e| match e {
                AuthServiceError::DebuggingNotAllowed => DebugServiceError::Unauthorized {
                    message: e.to_safe_string(),
                },
                e => DebugServiceError::internal(e.to_string(), Some(worker_id.clone())),
            })?;

        let owned_worker_id = OwnedWorkerId::new(&component.environment_id, worker_id);

        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        if self.debug_session.get(&debug_session_id).await.is_some() {
            return Err(DebugServiceError::conflict(
                worker_id.clone(),
                "Worker is already being debugged".to_string(),
            ));
        }

        // This simply migrates the worker to the debug mode, but it doesn't start the worker
        let (worker_metadata, worker_event_receiver) = self
            .connect_worker(
                worker_id.clone(),
                &component.account_id,
                &component.environment_id,
            )
            .await?;

        self.debug_session
            .insert(
                debug_session_id,
                DebugSessionData {
                    worker_metadata,
                    target_oplog_index: None,
                    playback_overrides: PlaybackOverridesInternal::empty(),
                    current_oplog_index: OplogIndex::NONE,
                },
            )
            .await;

        let connect_result = ConnectResult {
            worker_id: worker_id.clone(),
            message: format!("Worker {worker_id} connected"),
        };

        Ok((
            connect_result,
            component.account_id,
            owned_worker_id,
            worker_event_receiver,
        ))
    }

    async fn playback(
        &self,
        owned_worker_id: &OwnedWorkerId,
        account_id: &AccountId,
        target_index: OplogIndex,
        playback_overrides: Option<Vec<PlaybackOverride>>,
        ensure_invocation_boundary: bool,
    ) -> Result<PlaybackResult, DebugServiceError> {
        if !target_index.is_defined() {
            return Err(DebugServiceError::ValidationFailed {
                worker_id: Some(owned_worker_id.worker_id.clone()),
                errors: vec![format!(
                    "Trying to rewind to an invalid oplog index {target_index}"
                )],
            });
        }

        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());
        let worker_id = owned_worker_id.worker_id.clone();

        let session_data =
            self.debug_session
                .get(&debug_session_id)
                .await
                .ok_or(DebugServiceError::internal(
                    "No debug session found".to_string(),
                    Some(worker_id.clone()),
                ))?;

        let current_oplog_index = session_data.current_oplog_index;

        debug!("Playback from current oplog index {current_oplog_index}");

        // At this point, the worker do exist after the connect
        // however, the debug session is updated with a different target index
        // allowing replaying to (potentially) stop at this index
        let worker = Worker::get_or_create_suspended(
            &self.all,
            account_id,
            owned_worker_id,
            Some(session_data.worker_metadata.env.clone()),
            Some(session_data.worker_metadata.wasi_config_vars.clone()),
            Some(
                session_data
                    .worker_metadata
                    .last_known_status
                    .component_revision,
            ),
            session_data.worker_metadata.parent.clone(),
            &InvocationContextStack::fresh(),
        )
        .await
        .map_err(|e| DebugServiceError::internal(e.to_string(), Some(worker_id.clone())))?;

        // We select a new target index based on the given target index
        // such that it is always in an invocation boundary
        let new_target_index = if ensure_invocation_boundary {
            Self::target_index_at_invocation_boundary(&worker_id, &worker, target_index).await?
        } else {
            target_index
        };

        if new_target_index < current_oplog_index {
            return Err(DebugServiceError::internal(
                format!(
                    "Target oplog index {target_index} for playback is less than the existing target oplog index {current_oplog_index}. Use rewind instead"
                ),
                Some(debug_session_id.worker_id()),
            ));
        }

        let playback_overrides_validated = if let Some(overrides) = playback_overrides {
            Some(
                Self::validate_playback_overrides(
                    worker_id.clone(),
                    current_oplog_index,
                    overrides,
                )
                .await?,
            )
        } else {
            None
        };

        // We update the session with the new target index
        // before starting the worker
        self.debug_session
            .update(
                debug_session_id.clone(),
                new_target_index,
                playback_overrides_validated,
            )
            .await;

        // this will fail if the worker is not currently running and do nothing.
        // If this succeeded it means we continued from the previous oplog and only some of the log events are reemitted.
        let incremental_playback = worker.resume_replay().await.is_ok();

        // the worker was not running, we need to start it so it starts replaying
        if !incremental_playback {
            Worker::start_if_needed(worker.clone()).await.map_err(|e| {
                DebugServiceError::internal(
                    format!("Failed to start worker for resumption: {e}"),
                    Some(worker_id.clone()),
                )
            })?;
        }

        // This might fail if we are replaying beyond the oplog index and trapping due to entering live mode, ignore.
        let _ = worker.await_ready_to_process_commands().await;

        let stopped_at_index = self
            .debug_session
            .get(&debug_session_id)
            .await
            .map(|d| d.current_oplog_index)
            .unwrap_or(OplogIndex::INITIAL);

        Ok(PlaybackResult {
            worker_id: owned_worker_id.worker_id.clone(),
            current_index: stopped_at_index,
            incremental_playback,
            message: format!(
                "Playback worker {} stopped at index {}",
                owned_worker_id.worker_id, stopped_at_index
            ),
        })
    }

    async fn rewind(
        &self,
        owned_worker_id: &OwnedWorkerId,
        account_id: &AccountId,
        target_index: OplogIndex,
        ensure_invocation_boundary: bool,
    ) -> Result<RewindResult, DebugServiceError> {
        if !target_index.is_defined() {
            return Err(DebugServiceError::ValidationFailed {
                worker_id: Some(owned_worker_id.worker_id.clone()),
                errors: vec![format!(
                    "Trying to rewind to an invalid oplog index {target_index}"
                )],
            });
        }

        info!(
            "Rewinding worker {} to index {}",
            owned_worker_id.worker_id, target_index
        );

        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        let debug_session_data =
            self.debug_session
                .get(&debug_session_id)
                .await
                .ok_or(DebugServiceError::internal(
                    "No debug session found. Rewind cannot be called ".to_string(),
                    Some(owned_worker_id.worker_id.clone()),
                ))?;

        let current_oplog_index = debug_session_data.current_oplog_index;

        let worker = Worker::get_or_create_suspended(
            &self.all,
            account_id,
            owned_worker_id,
            Some(debug_session_data.worker_metadata.env.clone()),
            Some(debug_session_data.worker_metadata.wasi_config_vars.clone()),
            Some(
                debug_session_data
                    .worker_metadata
                    .last_known_status
                    .component_revision,
            ),
            debug_session_data.worker_metadata.parent.clone(),
            &InvocationContextStack::fresh(),
        )
        .await
        .map_err(|e| {
            DebugServiceError::internal(e.to_string(), Some(owned_worker_id.worker_id.clone()))
        })?;

        let new_target_index = if ensure_invocation_boundary {
            Self::get_target_oplog_index_at_invocation_boundary(
                worker.oplog(),
                target_index,
                current_oplog_index,
            )
            .await
            .map_err(|e| DebugServiceError::internal(e, Some(owned_worker_id.worker_id.clone())))?
        } else {
            target_index
        };

        if new_target_index >= current_oplog_index {
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
            .update_oplog_index(&debug_session_id, OplogIndex::NONE)
            .await;

        // we restart regardless of the current status of the worker such that it restarts
        if let Some(mut receiver) = worker.set_interrupting(InterruptKind::Restart).await {
            let _ = receiver.recv().await;
        };

        let _ = worker.await_ready_to_process_commands().await;

        let stopped_at_index = self
            .debug_session
            .get(&debug_session_id)
            .await
            .map(|d| d.current_oplog_index)
            .unwrap_or(OplogIndex::NONE);

        Ok(RewindResult {
            worker_id: owned_worker_id.worker_id.clone(),
            current_index: stopped_at_index,
            message: format!("Rewinding the worker to index {target_index}"),
        })
    }

    async fn fork(
        &self,
        account_id: &AccountId,
        source_worker_id: &OwnedWorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<ForkResult, DebugServiceError> {
        info!(
            "Forking worker {} to new worker {}",
            source_worker_id.worker_id, target_worker_id
        );

        // TODO: authorize here

        // Fork internally proxies the resume of worker using worker-proxy
        // making sure the worker is initiated in the regular worker executor, and not
        // debugging executor
        self.all
            .worker_fork_service()
            .fork(
                account_id,
                source_worker_id,
                target_worker_id,
                oplog_index_cut_off,
            )
            .await
            .map_err(|e| {
                DebugServiceError::internal(e.to_string(), Some(source_worker_id.worker_id.clone()))
            })?;

        Ok(ForkResult {
            source_worker_id: source_worker_id.worker_id.clone(),
            target_worker_id: target_worker_id.clone(),
            message: format!("Forked worker {source_worker_id} to new worker {target_worker_id}"),
        })
    }

    async fn current_oplog_index(
        &self,
        worker_id: &OwnedWorkerId,
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
                Some(worker_id.worker_id()),
            )),
        }
    }

    async fn terminate_session(
        &self,
        owned_worker_id: &OwnedWorkerId,
    ) -> Result<(), DebugServiceError> {
        let debug_session_id = DebugSessionId::new(owned_worker_id.clone());

        self.debug_session
            .remove(debug_session_id)
            .await
            .ok_or(DebugServiceError::internal(
                "No debug session found".to_string(),
                Some(owned_worker_id.worker_id()),
            ))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::{OplogEntry, OplogPayload, PayloadId, RawOplogPayload};
    use golem_common::model::oplog::{OplogIndex, PersistenceLevel};
    use golem_common::model::Timestamp;
    use golem_worker_executor::services::oplog::CommitLevel;
    use std::collections::BTreeMap;
    use std::fmt::{Debug, Formatter};
    use std::time::Duration;
    use test_r::test;

    #[test]
    async fn test_get_target_oplog_index_at_invocation_boundary_1() {
        let target_oplog_index = OplogIndex::from_u64(1);
        let original_last_oplog_index = OplogIndex::from_u64(10);

        let result = DebugServiceDefault::get_target_oplog_index_at_invocation_boundary(
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

        let result = DebugServiceDefault::get_target_oplog_index_at_invocation_boundary(
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
        async fn add(&self, _entry: OplogEntry) -> OplogIndex {
            unimplemented!()
        }

        async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
            unimplemented!()
        }

        async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
            unimplemented!()
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            unimplemented!()
        }

        async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
            unimplemented!()
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            unimplemented!()
        }

        async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
            if oplog_index == OplogIndex::from_u64(self.invocation_completion_index) {
                OplogEntry::ExportedFunctionCompleted {
                    timestamp: Timestamp::now_utc(),
                    response: OplogPayload::Inline(Box::new(None)),
                    consumed_fuel: 0,
                }
            } else {
                // Any other oplog entry other than export function completed
                OplogEntry::NoOp {
                    timestamp: Timestamp::now_utc(),
                }
            }
        }

        async fn read_many(
            &self,
            oplog_index: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            let mut result = BTreeMap::new();
            let mut current = oplog_index;
            for _ in 0..n {
                result.insert(current, self.read(current).await);
                current = current.next();
            }
            result
        }

        async fn length(&self) -> u64 {
            unimplemented!()
        }

        async fn upload_raw_payload(&self, _data: Vec<u8>) -> Result<RawOplogPayload, String> {
            unimplemented!()
        }

        async fn download_raw_payload(
            &self,
            _payload_id: PayloadId,
            _md5_hash: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            unimplemented!()
        }

        async fn switch_persistence_level(&self, _mode: PersistenceLevel) {}
    }
}
