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

use crate::debug_context::DebugContext;
use crate::debug_session::{
    DebugSessionData, DebugSessionId, DebugSessions, PlaybackOverridesInternal,
    validate_override_preserves_pairing,
};
use crate::model::params::*;
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_common::model::account::AccountId;
use golem_common::model::agent::Principal;
use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
use golem_common::model::card::{
    AgentResourcePattern, AgentVerb, ClassPermissionTarget, PermissionTarget,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AgentId, AgentMetadata, OwnedAgentId};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
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
        source_agent_id: &AgentId,
    ) -> Result<(ConnectResult, AccountId, OwnedAgentId, WorkerEventReceiver), DebugServiceError>;

    async fn playback(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_index: OplogIndex,
        overrides: Option<Vec<PlaybackOverride>>,
        ensure_invocation_boundary: bool,
    ) -> Result<PlaybackResult, DebugServiceError>;

    async fn rewind(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_index: OplogIndex,
        ensure_invocation_boundary: bool,
    ) -> Result<RewindResult, DebugServiceError>;

    async fn fork(
        &self,
        account_id: AccountId,
        source_owned_agent_id: &OwnedAgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        auth_ctx: &AuthCtx,
    ) -> Result<ForkResult, DebugServiceError>;

    async fn current_oplog_index(
        &self,
        agent_id: &OwnedAgentId,
    ) -> Result<OplogIndex, DebugServiceError>;

    async fn terminate_session(&self, agent_id: &OwnedAgentId) -> Result<(), DebugServiceError>;
}

#[derive(Clone, Debug)]
pub enum DebugServiceError {
    Internal {
        agent_id: Option<AgentId>,
    },
    Unauthorized {
        message: String,
    },
    Conflict {
        agent_id: AgentId,
        message: String,
    },
    ValidationFailed {
        agent_id: Option<AgentId>,
        errors: Vec<String>,
    },
}

impl DebugServiceError {
    pub fn conflict(agent_id: AgentId, message: String) -> Self {
        DebugServiceError::Conflict { agent_id, message }
    }

    pub fn unauthorized(message: String) -> Self {
        DebugServiceError::Unauthorized { message }
    }

    pub fn internal(message: String, agent_id: Option<AgentId>) -> Self {
        tracing::warn!("internal error in debugging service: {message}");
        DebugServiceError::Internal { agent_id }
    }

    pub fn validation_failed(errors: Vec<String>, agent_id: Option<AgentId>) -> Self {
        DebugServiceError::ValidationFailed { errors, agent_id }
    }

    pub fn get_agent_id(&self) -> Option<AgentId> {
        match self {
            DebugServiceError::Internal { agent_id, .. } => (*agent_id).clone(),
            DebugServiceError::Unauthorized { .. } => None,
            DebugServiceError::Conflict { agent_id, .. } => Some(agent_id.clone()),
            DebugServiceError::ValidationFailed { agent_id, .. } => (*agent_id).clone(),
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
    all: All<DebugContext>,
}

impl DebugServiceDefault {
    pub fn new(all: All<DebugContext>) -> Self {
        let component_service = all.component_service();
        let extra_deps = all.extra_deps();
        let debug_session = extra_deps.debug_session();

        Self {
            component_service,
            debug_session,
            all,
        }
    }

    // Launches/migrate the worker to the debug mode
    // First step is to get the worker details that's currently being run
    async fn connect_worker(
        &self,
        agent_id: AgentId,
        enironment_id: EnvironmentId,
    ) -> Result<(AgentMetadata, WorkerEventReceiver), DebugServiceError> {
        let owned_agent_id = OwnedAgentId::new(enironment_id, &agent_id);

        // This get will only look at the oplogs to see if a worker presumably exists in the real executor.
        // This is only used to get the existing metadata that was/is running in the real executor
        self.all
            .worker_service()
            .get(&owned_agent_id)
            .await
            .ok_or_else(|| {
                DebugServiceError::conflict(
                    agent_id.clone(),
                    "Worker doesn't exist in live/real worker executor for it to connect to"
                        .to_string(),
                )
            })?;

        let port = self.all.config().grpc.port;

        info!("Registering worker {} with port {}", agent_id, port);

        let shard_assignment = self
            .all
            .shard_manager_service()
            .register(port, None)
            .await
            .map_err(|e| DebugServiceError::internal(e.to_string(), Some(agent_id.clone())))?;

        self.all.shard_service().register(
            shard_assignment.number_of_shards,
            &shard_assignment.shard_ids,
        );

        let worker = Worker::get_or_create_suspended(
            &self.all,
            &owned_agent_id,
            None,
            Vec::new(),
            None,
            None,
            &InvocationContextStack::fresh(),
            Principal::anonymous(),
        )
        .await
        .map_err(|e| DebugServiceError::internal(e.to_string(), Some(agent_id.clone())))?;

        let metadata = worker.get_latest_worker_metadata().await;

        let receiver = worker.event_service().receiver();

        Ok((metadata, receiver))
    }

    pub async fn validate_playback_overrides(
        agent_id: AgentId,
        current_index: OplogIndex,
        overrides: Vec<PlaybackOverride>,
        raw_oplog: Arc<dyn Oplog>,
    ) -> Result<PlaybackOverridesInternal, DebugServiceError> {
        let validated = PlaybackOverridesInternal::from_playback_override(overrides, current_index)
            .map_err(|err| DebugServiceError::ValidationFailed {
                agent_id: Some(agent_id.clone()),
                errors: vec![err],
            })?;

        // Replay resolves paired durable constructs (`Start`/`End`/`Cancelled`, atomic regions,
        // remote transactions) by oplog index, so each override must preserve the pairing
        // signature of the entry it replaces or the replay resolver state gets corrupted.
        let last_recorded_index = raw_oplog.current_oplog_index().await;
        let mut errors = Vec::new();
        let mut indices: Vec<OplogIndex> = validated.overrides.keys().copied().collect();
        indices.sort();
        for index in indices {
            if index > last_recorded_index {
                errors.push(format!(
                    "Playback override at oplog index {index} is beyond the recorded oplog (last recorded index is {last_recorded_index})"
                ));
                continue;
            }
            let underlying = raw_oplog.read(index).await;
            if let Err(err) = validate_override_preserves_pairing(
                index,
                &underlying,
                &validated.overrides[&index],
                &raw_oplog,
            )
            .await
            {
                errors.push(err);
            }
        }

        if errors.is_empty() {
            Ok(validated)
        } else {
            Err(DebugServiceError::ValidationFailed {
                agent_id: Some(agent_id),
                errors,
            })
        }
    }

    /// Scans the recorded oplog up to `target_index` and returns the first durable call `Start`
    /// that has no matching terminal (`End`/`Cancelled`) at or before the target — that is, a
    /// durable call that is still in flight at the target index. Regions dropped by `Jump` or
    /// `Revert` entries are excluded, mirroring how replay skips them.
    ///
    /// A debug session must refuse to start playback at such a target: replay would either
    /// resolve the call from entries beyond the target or live re-execute it, and debug sessions
    /// never live-repair incomplete durable calls
    /// (`DebugContext::ALLOW_LIVE_REPAIR_OF_INCOMPLETE_DURABLE_CALLS` is `false`).
    pub async fn find_in_flight_durable_call_at(
        raw_oplog: Arc<dyn Oplog>,
        target_index: OplogIndex,
    ) -> Option<(OplogIndex, String)> {
        const CHUNK_SIZE: u64 = 1024;

        let scan_end = target_index.min(raw_oplog.current_oplog_index().await);
        let mut unmatched: std::collections::BTreeMap<OplogIndex, String> =
            std::collections::BTreeMap::new();

        let mut index = OplogIndex::INITIAL;
        while index <= scan_end {
            let available = u64::from(scan_end) - u64::from(index) + 1;
            let entries = raw_oplog.read_many(index, CHUNK_SIZE.min(available)).await;
            if entries.is_empty() {
                break;
            }
            for (idx, entry) in &entries {
                match entry {
                    OplogEntry::Start { function_name, .. } => {
                        unmatched.insert(*idx, function_name.to_string());
                    }
                    OplogEntry::End { start_index, .. }
                    | OplogEntry::Cancelled { start_index, .. } => {
                        unmatched.remove(start_index);
                    }
                    OplogEntry::Jump { jump, .. } => {
                        unmatched.retain(|start, _| !jump.contains(*start));
                    }
                    OplogEntry::Revert { dropped_region, .. } => {
                        unmatched.retain(|start, _| !dropped_region.contains(*start));
                    }
                    _ => {}
                }
            }
            index = entries.last_key_value().map(|(idx, _)| idx.next())?;
        }

        unmatched.pop_first()
    }

    /// Scans the full recorded oplog and returns the first durable call whose `End` lies at or
    /// before `target_index` while its `CompletionDiscarded` marker lies strictly after it —
    /// that is, a target that splits a call's completion from its recorded delivery status.
    /// Returns `(start_index, end_index, marker_index)` for the offending call.
    ///
    /// Replaying to such a target would deliver the completion to the guest, but the recorded
    /// execution never observed it (the marker records that it was discarded before delivery),
    /// so every recorded entry after the `End` reflects the discarded outcome. A debug session
    /// parked at such a target would diverge from the recording as soon as its target advances
    /// past the marker, so the target is rejected up front.
    ///
    /// Regions dropped by `Jump` or `Revert` entries are excluded on both sides, mirroring how
    /// replay skips them: a marker in a dropped region never influences replay, and an `End` in
    /// a dropped region is never resolved from.
    pub async fn find_split_discarded_completion_at(
        raw_oplog: Arc<dyn Oplog>,
        target_index: OplogIndex,
    ) -> Option<(OplogIndex, OplogIndex, OplogIndex)> {
        const CHUNK_SIZE: u64 = 1024;

        let scan_end = raw_oplog.current_oplog_index().await;
        if target_index >= scan_end {
            return None;
        }

        // start_index -> End index, for End entries at or before the target
        let mut ends: std::collections::BTreeMap<OplogIndex, OplogIndex> =
            std::collections::BTreeMap::new();
        // marker index -> start_index, for CompletionDiscarded entries after the target
        let mut markers: std::collections::BTreeMap<OplogIndex, OplogIndex> =
            std::collections::BTreeMap::new();

        let mut index = OplogIndex::INITIAL;
        while index <= scan_end {
            let available = u64::from(scan_end) - u64::from(index) + 1;
            let entries = raw_oplog.read_many(index, CHUNK_SIZE.min(available)).await;
            if entries.is_empty() {
                break;
            }
            for (idx, entry) in &entries {
                match entry {
                    OplogEntry::End { start_index, .. } if *idx <= target_index => {
                        ends.insert(*start_index, *idx);
                    }
                    OplogEntry::CompletionDiscarded { start_index, .. } if *idx > target_index => {
                        markers.insert(*idx, *start_index);
                    }
                    OplogEntry::Jump { jump, .. } => {
                        ends.retain(|start, end| !jump.contains(*start) && !jump.contains(*end));
                        markers.retain(|marker, _| !jump.contains(*marker));
                    }
                    OplogEntry::Revert { dropped_region, .. } => {
                        ends.retain(|start, end| {
                            !dropped_region.contains(*start) && !dropped_region.contains(*end)
                        });
                        markers.retain(|marker, _| !dropped_region.contains(*marker));
                    }
                    _ => {}
                }
            }
            index = entries.last_key_value().map(|(idx, _)| idx.next())?;
        }

        markers.into_iter().find_map(|(marker_idx, start_idx)| {
            ends.get(&start_idx)
                .map(|end_idx| (start_idx, *end_idx, marker_idx))
        })
    }

    pub async fn target_index_at_invocation_boundary(
        agent_id: &AgentId,
        worker: &Arc<Worker<DebugContext>>,
        target_oplog_index: OplogIndex,
    ) -> Result<OplogIndex, DebugServiceError> {
        // The scan must read the raw underlying oplog: reading through the worker's `DebugOplog`
        // would apply playback overrides and move the debug session's current index, and its
        // `current_oplog_index` reports the session's (possibly stale) playback target instead of
        // the recorded oplog end.
        let debug_oplog = worker.oplog();
        let oplog: Arc<dyn Oplog> = debug_oplog.inner().unwrap_or(debug_oplog);

        let original_current_oplog_index = oplog.current_oplog_index().await;

        Self::get_target_oplog_index_at_invocation_boundary(
            oplog,
            target_oplog_index,
            original_current_oplog_index,
        )
        .await
        .map_err(|e| DebugServiceError::internal(e, Some(agent_id.clone())))
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
                OplogEntry::AgentInvocationFinished { .. } => {
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
        agent_id: &AgentId,
    ) -> Result<(ConnectResult, AccountId, OwnedAgentId, WorkerEventReceiver), DebugServiceError>
    {
        let component = self
            .component_service
            .get_metadata(agent_id.component_id, None)
            .await
            .map_err(|e| DebugServiceError::internal(e.to_string(), Some(agent_id.clone())))?;

        authorize_debugging(auth_ctx, &component, agent_id)?;

        let owned_agent_id = OwnedAgentId::new(component.environment_id, agent_id);

        let debug_session_id = DebugSessionId::new(owned_agent_id.clone());

        if self.debug_session.get(&debug_session_id).await.is_some() {
            return Err(DebugServiceError::conflict(
                agent_id.clone(),
                "Worker is already being debugged".to_string(),
            ));
        }

        // This simply migrates the worker to the debug mode, but it doesn't start the worker
        let (worker_metadata, worker_event_receiver) = self
            .connect_worker(agent_id.clone(), component.environment_id)
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
            agent_id: agent_id.clone(),
            message: format!("Worker {agent_id} connected"),
        };

        Ok((
            connect_result,
            component.account_id,
            owned_agent_id,
            worker_event_receiver,
        ))
    }

    async fn playback(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_index: OplogIndex,
        playback_overrides: Option<Vec<PlaybackOverride>>,
        ensure_invocation_boundary: bool,
    ) -> Result<PlaybackResult, DebugServiceError> {
        if !target_index.is_defined() {
            return Err(DebugServiceError::ValidationFailed {
                agent_id: Some(owned_agent_id.agent_id.clone()),
                errors: vec![format!(
                    "Trying to rewind to an invalid oplog index {target_index}"
                )],
            });
        }

        let debug_session_id = DebugSessionId::new(owned_agent_id.clone());
        let agent_id = owned_agent_id.agent_id.clone();

        let session_data =
            self.debug_session
                .get(&debug_session_id)
                .await
                .ok_or(DebugServiceError::internal(
                    "No debug session found".to_string(),
                    Some(agent_id.clone()),
                ))?;

        let current_oplog_index = session_data.current_oplog_index;

        debug!("Playback from current oplog index {current_oplog_index}");

        // At this point, the worker do exist after the connect
        // however, the debug session is updated with a different target index
        // allowing replaying to (potentially) stop at this index
        let worker = Worker::get_or_create_suspended(
            &self.all,
            owned_agent_id,
            None,
            Vec::new(),
            None,
            None,
            &InvocationContextStack::fresh(),
            Principal::anonymous(),
        )
        .await
        .map_err(|e| DebugServiceError::internal(e.to_string(), Some(agent_id.clone())))?;

        // We select a new target index based on the given target index
        // such that it is always in an invocation boundary
        let new_target_index = if ensure_invocation_boundary {
            Self::target_index_at_invocation_boundary(&agent_id, &worker, target_index).await?
        } else {
            target_index
        };

        if new_target_index < current_oplog_index {
            return Err(DebugServiceError::internal(
                format!(
                    "Target oplog index {target_index} for playback is less than the existing target oplog index {current_oplog_index}. Use rewind instead"
                ),
                Some(debug_session_id.agent_id()),
            ));
        }

        // The raw underlying oplog: reading through the worker's `DebugOplog` would apply
        // existing overrides and move the session's current index.
        let raw_oplog = {
            let debug_oplog = worker.oplog();
            debug_oplog.inner().unwrap_or(debug_oplog)
        };

        // Refuse targets that fall inside an in-flight durable call: replay could neither
        // resolve the call from the entries before the target nor safely re-execute it live
        // (debug sessions never live-repair incomplete durable calls, as re-executing the
        // recorded side effects would not be a faithful playback).
        if let Some((start_index, function_name)) =
            Self::find_in_flight_durable_call_at(raw_oplog.clone(), new_target_index).await
        {
            return Err(DebugServiceError::ValidationFailed {
                agent_id: Some(agent_id.clone()),
                errors: vec![format!(
                    "Playback target index {new_target_index} is inside an in-flight durable call: {function_name} started at oplog index {start_index} and completes after the target. Debug sessions refuse live repair or re-execution of incomplete durable calls; choose a target index at or after the call's completion"
                )],
            });
        }

        // Refuse targets that split a completed durable call's `End` from its
        // `CompletionDiscarded` marker: replay to such a target would deliver a completion the
        // recorded execution never observed, diverging from the recording as soon as the target
        // advances past the marker.
        if let Some((start_index, end_index, marker_index)) =
            Self::find_split_discarded_completion_at(raw_oplog.clone(), new_target_index).await
        {
            return Err(DebugServiceError::ValidationFailed {
                agent_id: Some(agent_id.clone()),
                errors: vec![format!(
                    "Playback target index {new_target_index} splits a durable call's completion from its recorded delivery status: the call started at oplog index {start_index} completed at {end_index}, but its completion was discarded before reaching the guest (CompletionDiscarded marker at {marker_index}). Replaying to this target would deliver a completion the recorded execution never observed; choose a target index before the call's completion or at/after the marker"
                )],
            });
        }

        let playback_overrides_validated = if let Some(overrides) = playback_overrides {
            Some(
                Self::validate_playback_overrides(
                    agent_id.clone(),
                    current_oplog_index,
                    overrides,
                    raw_oplog,
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
                    Some(agent_id.clone()),
                )
            })?;
        }

        // This might fail if we are replaying beyond the oplog index and trapping due to entering live mode.
        let ready_result = worker.await_ready_to_process_commands().await;

        let stopped_at_index = self
            .debug_session
            .get(&debug_session_id)
            .await
            .map(|d| d.current_oplog_index)
            .unwrap_or(OplogIndex::INITIAL);

        // A replay failure before reaching the target is an error, not a successful playback (for
        // example a replay target inside an in-flight durable call, whose live repair is refused
        // in debug sessions). Failures at or past the target are the expected trap when playback
        // runs off the end of the recorded oplog into live mode, and stay ignored.
        if let Err(error) = &ready_result
            && stopped_at_index < new_target_index
        {
            return Err(DebugServiceError::validation_failed(
                vec![format!(
                    "Playback worker {} stopped at index {} before reaching target index {}: {}",
                    owned_agent_id.agent_id, stopped_at_index, new_target_index, error
                )],
                Some(agent_id.clone()),
            ));
        }

        Ok(PlaybackResult {
            agent_id: owned_agent_id.agent_id.clone(),
            current_index: stopped_at_index,
            incremental_playback,
            message: format!(
                "Playback worker {} stopped at index {}",
                owned_agent_id.agent_id, stopped_at_index
            ),
        })
    }

    async fn rewind(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_index: OplogIndex,
        ensure_invocation_boundary: bool,
    ) -> Result<RewindResult, DebugServiceError> {
        if !target_index.is_defined() {
            return Err(DebugServiceError::ValidationFailed {
                agent_id: Some(owned_agent_id.agent_id.clone()),
                errors: vec![format!(
                    "Trying to rewind to an invalid oplog index {target_index}"
                )],
            });
        }

        info!(
            "Rewinding worker {} to index {}",
            owned_agent_id.agent_id, target_index
        );

        let debug_session_id = DebugSessionId::new(owned_agent_id.clone());

        let debug_session_data =
            self.debug_session
                .get(&debug_session_id)
                .await
                .ok_or(DebugServiceError::internal(
                    "No debug session found. Rewind cannot be called ".to_string(),
                    Some(owned_agent_id.agent_id.clone()),
                ))?;

        let current_oplog_index = debug_session_data.current_oplog_index;

        let worker = Worker::get_or_create_suspended(
            &self.all,
            owned_agent_id,
            None,
            Vec::new(),
            None,
            None,
            &InvocationContextStack::fresh(),
            Principal::anonymous(),
        )
        .await
        .map_err(|e| {
            DebugServiceError::internal(e.to_string(), Some(owned_agent_id.agent_id.clone()))
        })?;

        // The raw underlying oplog: reading through the worker's `DebugOplog` would apply
        // playback overrides, and its `current_oplog_index` reports the session's playback
        // target instead of the recorded oplog end.
        let raw_oplog = {
            let debug_oplog = worker.oplog();
            debug_oplog.inner().unwrap_or(debug_oplog)
        };

        let new_target_index = if ensure_invocation_boundary {
            Self::get_target_oplog_index_at_invocation_boundary(
                raw_oplog.clone(),
                target_index,
                current_oplog_index,
            )
            .await
            .map_err(|e| DebugServiceError::internal(e, Some(owned_agent_id.agent_id.clone())))?
        } else {
            target_index
        };

        if new_target_index >= current_oplog_index {
            return Err(DebugServiceError::validation_failed(
                vec![format!(
                    "Target oplog index {} (corresponding to an invocation boundary) for rewind is greater than the existing target oplog index {}",
                    target_index, current_oplog_index
                )],
                Some(owned_agent_id.agent_id.clone()),
            ));
        };

        // Refuse targets that fall inside an in-flight durable call, exactly like playback:
        // replay could neither resolve the call from the entries before the target nor safely
        // re-execute it live (debug sessions never live-repair incomplete durable calls).
        if let Some((start_index, function_name)) =
            Self::find_in_flight_durable_call_at(raw_oplog.clone(), new_target_index).await
        {
            return Err(DebugServiceError::ValidationFailed {
                agent_id: Some(owned_agent_id.agent_id.clone()),
                errors: vec![format!(
                    "Rewind target index {new_target_index} is inside an in-flight durable call: {function_name} started at oplog index {start_index} and completes after the target. Debug sessions refuse live repair or re-execution of incomplete durable calls; choose a target index at or after the call's completion"
                )],
            });
        }

        // Refuse targets that split a completed durable call's `End` from its
        // `CompletionDiscarded` marker, exactly like playback: replay to such a target would
        // deliver a completion the recorded execution never observed.
        if let Some((start_index, end_index, marker_index)) =
            Self::find_split_discarded_completion_at(raw_oplog.clone(), new_target_index).await
        {
            return Err(DebugServiceError::ValidationFailed {
                agent_id: Some(owned_agent_id.agent_id.clone()),
                errors: vec![format!(
                    "Rewind target index {new_target_index} splits a durable call's completion from its recorded delivery status: the call started at oplog index {start_index} completed at {end_index}, but its completion was discarded before reaching the guest (CompletionDiscarded marker at {marker_index}). Replaying to this target would deliver a completion the recorded execution never observed; choose a target index before the call's completion or at/after the marker"
                )],
            });
        }

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

        // `set_interrupting` is a no-op when the worker is already unloaded (for example it
        // suspended and unloaded itself after a previous playback), so the worker must be
        // explicitly started to run the rewind replay. This is harmless when the restart above
        // already left it running.
        Worker::start_if_needed(worker.clone()).await.map_err(|e| {
            DebugServiceError::internal(
                format!("Failed to start worker for rewind: {e}"),
                Some(owned_agent_id.agent_id.clone()),
            )
        })?;

        let ready_result = worker.await_ready_to_process_commands().await;

        let stopped_at_index = self
            .debug_session
            .get(&debug_session_id)
            .await
            .map(|d| d.current_oplog_index)
            .unwrap_or(OplogIndex::NONE);

        // A rewind target always lies inside the recorded oplog (it is strictly before the
        // session's previous position), so a successful rewind must replay exactly to it;
        // anything else is a failed rewind, not a success.
        if stopped_at_index != new_target_index {
            let reason = match &ready_result {
                Err(error) => format!(": {error}"),
                Ok(()) => String::new(),
            };
            return Err(DebugServiceError::validation_failed(
                vec![format!(
                    "Rewind worker {} stopped at index {} instead of the target index {}{}",
                    owned_agent_id.agent_id, stopped_at_index, new_target_index, reason
                )],
                Some(owned_agent_id.agent_id.clone()),
            ));
        }

        Ok(RewindResult {
            agent_id: owned_agent_id.agent_id.clone(),
            current_index: stopped_at_index,
            message: format!("Rewinding the worker to index {target_index}"),
        })
    }

    async fn fork(
        &self,
        account_id: AccountId,
        source_agent_id: &OwnedAgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        auth_ctx: &AuthCtx,
    ) -> Result<ForkResult, DebugServiceError> {
        info!(
            "Forking worker {} to new worker {}",
            source_agent_id.agent_id, target_agent_id
        );

        // Fork internally proxies the resume of worker using worker-proxy
        // making sure the worker is initiated in the regular worker executor, and not
        // debugging executor
        self.all
            .worker_fork_service()
            .fork(
                account_id,
                source_agent_id,
                target_agent_id,
                oplog_index_cut_off,
                auth_ctx,
            )
            .await
            .map_err(|e| {
                DebugServiceError::internal(e.to_string(), Some(source_agent_id.agent_id.clone()))
            })?;

        Ok(ForkResult {
            source_agent_id: source_agent_id.agent_id.clone(),
            target_agent_id: target_agent_id.clone(),
            message: format!("Forked worker {source_agent_id} to new worker {target_agent_id}"),
        })
    }

    async fn current_oplog_index(
        &self,
        agent_id: &OwnedAgentId,
    ) -> Result<OplogIndex, DebugServiceError> {
        let debug_session_id = DebugSessionId::new(agent_id.clone());

        let result = self
            .debug_session
            .get(&debug_session_id)
            .await
            .map(|debug_session| debug_session.current_oplog_index);

        match result {
            Some(index) => Ok(index),
            None => Err(DebugServiceError::internal(
                "No debug session found".to_string(),
                Some(agent_id.agent_id()),
            )),
        }
    }

    async fn terminate_session(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<(), DebugServiceError> {
        let debug_session_id = DebugSessionId::new(owned_agent_id.clone());

        self.debug_session
            .remove(debug_session_id)
            .await
            .ok_or(DebugServiceError::internal(
                "No debug session found".to_string(),
                Some(owned_agent_id.agent_id()),
            ))?;

        Ok(())
    }
}

fn authorize_debugging(
    auth_ctx: &AuthCtx,
    component: &Component,
    agent_id: &AgentId,
) -> Result<(), DebugServiceError> {
    auth_ctx
        .authorize_permission(&PermissionTarget::Agent(ClassPermissionTarget {
            owner: AgentOwnerPattern::Agent {
                account: component.account_email.clone(),
                application: component.application_name.clone(),
                environment: component.environment_name.clone(),
                component: component.component_name.clone(),
                agent: AgentOwnerLeafPattern::Agent(agent_id.agent_id.clone()),
            },
            verb: Some(AgentVerb::Debug),
            resource: AgentResourcePattern::Any,
        }))
        .map_err(|e| DebugServiceError::Unauthorized {
            message: e.to_safe_string(),
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::component::ComponentRevision;
    use golem_common::model::oplog::{OplogEntry, OplogPayload, PayloadId, RawOplogPayload};
    use golem_common::model::oplog::{OplogIndex, PersistenceLevel};
    use golem_common::model::regions::OplogRegion;
    use golem_common::model::{AgentInvocationResult, Timestamp};
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

    fn noop_entry() -> OplogEntry {
        OplogEntry::NoOp {
            timestamp: Timestamp::now_utc(),
        }
    }

    fn end_entry(start_index: u64) -> OplogEntry {
        OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start_index),
            response: None,
            forced_commit: false,
        }
    }

    fn discarded_entry(start_index: u64) -> OplogEntry {
        OplogEntry::CompletionDiscarded {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start_index),
        }
    }

    fn jump_entry(start: u64, end: u64) -> OplogEntry {
        OplogEntry::Jump {
            timestamp: Timestamp::now_utc(),
            jump: OplogRegion {
                start: OplogIndex::from_u64(start),
                end: OplogIndex::from_u64(end),
            },
        }
    }

    fn revert_entry(start: u64, end: u64) -> OplogEntry {
        OplogEntry::Revert {
            timestamp: Timestamp::now_utc(),
            dropped_region: OplogRegion {
                start: OplogIndex::from_u64(start),
                end: OplogIndex::from_u64(end),
            },
        }
    }

    async fn split_at(entries: Vec<OplogEntry>, target: u64) -> Option<(u64, u64, u64)> {
        DebugServiceDefault::find_split_discarded_completion_at(
            Arc::new(SeqOplog { entries }),
            OplogIndex::from_u64(target),
        )
        .await
        .map(|(start, end, marker)| (u64::from(start), u64::from(end), u64::from(marker)))
    }

    #[test]
    async fn split_discarded_completion_is_rejected_between_end_and_marker() {
        // [Start placeholder, End(1), NoOp, CompletionDiscarded(1)]
        let entries = vec![noop_entry(), end_entry(1), noop_entry(), discarded_entry(1)];
        // Targets strictly between the End (2) and the marker (4) split the pair
        assert_eq!(split_at(entries.clone(), 2).await, Some((1, 2, 4)));
        assert_eq!(split_at(entries.clone(), 3).await, Some((1, 2, 4)));
        // Target before the End: the completion is not visible at all, nothing splits
        assert_eq!(split_at(entries.clone(), 1).await, None);
        // Target at the marker: the marker is visible and replay parks the delivery
        assert_eq!(split_at(entries, 4).await, None);
    }

    #[test]
    async fn split_check_ignores_calls_completing_after_the_target() {
        // End and marker both lie after the target: the in-flight validator's domain, not a
        // split pair
        let entries = vec![noop_entry(), noop_entry(), end_entry(1), discarded_entry(1)];
        assert_eq!(split_at(entries, 2).await, None);
    }

    #[test]
    async fn split_check_ignores_markers_in_dropped_regions() {
        // The marker is inside a region dropped by a later Revert: replay never sees it
        let entries = vec![
            noop_entry(),
            end_entry(1),
            discarded_entry(1),
            revert_entry(3, 3),
        ];
        assert_eq!(split_at(entries, 2).await, None);
    }

    #[test]
    async fn split_check_ignores_calls_in_regions_dropped_by_jump() {
        // The call's Start and End are inside a region skipped by a Jump: the marker is an
        // orphan that replay drains without effect
        let entries = vec![
            noop_entry(),
            end_entry(1),
            jump_entry(1, 2),
            discarded_entry(1),
        ];
        assert_eq!(split_at(entries, 3).await, None);
    }

    #[test]
    async fn split_check_accepts_target_at_or_beyond_oplog_end() {
        let entries = vec![noop_entry(), end_entry(1), discarded_entry(1)];
        assert_eq!(split_at(entries.clone(), 3).await, None);
        assert_eq!(split_at(entries, 10).await, None);
    }

    struct SeqOplog {
        entries: Vec<OplogEntry>,
    }

    impl Debug for SeqOplog {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "SeqOplog")
        }
    }

    #[async_trait]
    impl Oplog for SeqOplog {
        async fn add(&self, _entry: OplogEntry) -> OplogIndex {
            unimplemented!()
        }

        async fn add_start_with_reserved_raw_payload(
            &self,
            _serialized_request: Vec<u8>,
            _build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
        ) -> Result<golem_worker_executor::services::oplog::OrderedOplogStart, String> {
            unimplemented!()
        }

        async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
            unimplemented!()
        }

        async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
            unimplemented!()
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            OplogIndex::from_u64(self.entries.len() as u64)
        }

        async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
            unimplemented!()
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            unimplemented!()
        }

        async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
            self.entries[(u64::from(oplog_index) - 1) as usize].clone()
        }

        async fn read_many(
            &self,
            oplog_index: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            let mut result = BTreeMap::new();
            let mut current = oplog_index;
            for _ in 0..n {
                if u64::from(current) > self.entries.len() as u64 {
                    break;
                }
                result.insert(current, self.read(current).await);
                current = current.next();
            }
            result
        }

        async fn length(&self) -> u64 {
            self.entries.len() as u64
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

        async fn add_start_with_reserved_raw_payload(
            &self,
            _serialized_request: Vec<u8>,
            _build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
        ) -> Result<golem_worker_executor::services::oplog::OrderedOplogStart, String> {
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
                OplogEntry::AgentInvocationFinished {
                    timestamp: Timestamp::now_utc(),
                    result: OplogPayload::Inline(Box::new(
                        AgentInvocationResult::AgentInitialization,
                    )),
                    method_name: None,
                    consumed_fuel: 0,
                    component_revision: ComponentRevision::INITIAL,
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
