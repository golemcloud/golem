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

use crate::model::params::PlaybackOverride;
use async_trait::async_trait;
use golem_common::model::agent::UntypedDataValue;
use golem_common::model::component::PluginPriority;
use golem_common::model::oplog::host_functions::{
    host_request_from_value_and_type, host_response_from_value_and_type, HostFunctionName,
};
use golem_common::model::oplog::public_oplog_entry::{
    AgentInvocationFinishedParams, AgentInvocationStartedParams, CreateParams,
    CreateResourceParams, DropResourceParams, FailedUpdateParams, GrowMemoryParams, HostCallParams,
    LogParams, PublicAgentInvocationResult, RawSnapshotData,
};
use golem_common::model::oplog::{
    DurableFunctionType, OplogEntry, OplogIndex, OplogPayload, WorkerError,
};
use golem_common::model::oplog::{PublicDurableFunctionType, PublicOplogEntry, PublicSnapshotData};
use golem_common::model::{
    AgentInvocationResult, OwnedWorkerId, RetryConfig, WorkerId, WorkerMetadata,
};
use golem_wasm::wasmtime::ResourceTypeId;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::sync::{Arc, Mutex};

// A shared debug session which will be internally used by the custom oplog service
// dedicated to running debug executor
#[async_trait]
pub trait DebugSessions: Send + Sync {
    async fn insert(
        &self,
        debug_session_id: DebugSessionId,
        session_value: DebugSessionData,
    ) -> DebugSessionId;
    async fn get(&self, debug_session_id: &DebugSessionId) -> Option<DebugSessionData>;

    async fn remove(&self, debug_session_id: DebugSessionId) -> Option<DebugSessionData>;

    async fn update(
        &self,
        debug_session_id: DebugSessionId,
        target_oplog_index: OplogIndex,
        playback_overrides: Option<PlaybackOverridesInternal>,
    ) -> Option<DebugSessionData>;

    async fn update_oplog_index(
        &self,
        debug_session_id: &DebugSessionId,
        oplog_index: OplogIndex,
    ) -> Option<DebugSessionData>;
}
pub struct DebugSessionsDefault {
    pub session: Arc<Mutex<HashMap<DebugSessionId, DebugSessionData>>>,
}

impl Default for DebugSessionsDefault {
    fn default() -> Self {
        Self {
            session: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl DebugSessions for DebugSessionsDefault {
    async fn insert(
        &self,
        debug_session_id: DebugSessionId,
        session_value: DebugSessionData,
    ) -> DebugSessionId {
        let mut session = self.session.lock().unwrap();
        session.insert(debug_session_id.clone(), session_value);
        debug_session_id
    }

    async fn get(&self, debug_session_id: &DebugSessionId) -> Option<DebugSessionData> {
        let session = self.session.lock().unwrap();
        session.get(debug_session_id).cloned()
    }

    async fn remove(&self, debug_session_id: DebugSessionId) -> Option<DebugSessionData> {
        let mut session = self.session.lock().unwrap();
        session.remove(&debug_session_id)
    }

    async fn update(
        &self,
        debug_session_id: DebugSessionId,
        target_oplog_index: OplogIndex,
        playback_overrides: Option<PlaybackOverridesInternal>,
    ) -> Option<DebugSessionData> {
        let mut session = self.session.lock().unwrap();
        let session_data = session.get_mut(&debug_session_id);
        if let Some(session_data) = session_data {
            session_data.target_oplog_index = Some(target_oplog_index);
            if let Some(playback_overrides) = playback_overrides {
                session_data.playback_overrides = playback_overrides
            }
            Some(session_data.clone())
        } else {
            None
        }
    }

    async fn update_oplog_index(
        &self,
        debug_session_id: &DebugSessionId,
        oplog_index: OplogIndex,
    ) -> Option<DebugSessionData> {
        let mut session = self.session.lock().unwrap();
        let session_data = session.get_mut(debug_session_id);
        if let Some(session_data) = session_data {
            session_data.current_oplog_index = oplog_index;
            Some(session_data.clone())
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub struct DebugSessionData {
    pub worker_metadata: WorkerMetadata,
    pub target_oplog_index: Option<OplogIndex>,
    pub playback_overrides: PlaybackOverridesInternal,
    // The current status of the oplog index being replayed and possibly
    // index of newly added oplog entries as part of going live in between host functions
    pub current_oplog_index: OplogIndex,
}

#[derive(Debug, Clone)]
pub struct PlaybackOverridesInternal {
    pub overrides: HashMap<OplogIndex, OplogEntry>,
}

impl PlaybackOverridesInternal {
    pub fn empty() -> PlaybackOverridesInternal {
        PlaybackOverridesInternal {
            overrides: HashMap::new(),
        }
    }
    pub fn from_playback_override(
        playback_overrides: Vec<PlaybackOverride>,
        current_index: OplogIndex,
    ) -> Result<Self, String> {
        let mut overrides = HashMap::new();
        for override_data in playback_overrides {
            let oplog_index = override_data.index;
            if oplog_index <= current_index {
                return Err(
                    "Cannot create overrides for oplog indices that are in the past".to_string(),
                );
            }

            let public_oplog_entry: PublicOplogEntry = override_data.oplog;
            let oplog_entry = get_oplog_entry_from_public_oplog_entry(public_oplog_entry)?;
            overrides.insert(oplog_index, oplog_entry);
        }
        Ok(Self { overrides })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DebugSessionId(WorkerId);

impl Serialize for DebugSessionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&self.0.to_string(), serializer)
    }
}

impl DebugSessionId {
    pub fn new(worker_id: OwnedWorkerId) -> Self {
        DebugSessionId(worker_id.worker_id)
    }

    pub fn worker_id(&self) -> WorkerId {
        self.0.clone()
    }
}
impl Display for DebugSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone)]
pub struct ActiveSessionData {
    // pub cloud_namespace: Namespace,
    pub worker_id: WorkerId,
}

impl ActiveSessionData {
    pub fn new(worker_id: WorkerId) -> Self {
        Self { worker_id }
    }
}

fn get_oplog_entry_from_public_oplog_entry(
    public_oplog_entry: PublicOplogEntry,
) -> Result<OplogEntry, String> {
    match public_oplog_entry {
        PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
            timestamp,
            consumed_fuel,
            result,
            component_revision,
        }) => {
            let raw_result = public_agent_invocation_result_to_raw(result)?;
            Ok(OplogEntry::AgentInvocationFinished {
                timestamp,
                result: OplogPayload::Inline(Box::new(raw_result)),
                consumed_fuel,
                component_revision,
            })
        }

        PublicOplogEntry::Create(CreateParams {
            timestamp,
            worker_id,
            component_revision,
            env,
            environment_id,
            created_by,
            config_vars,
            parent,
            component_size,
            initial_total_linear_memory_size,
            initial_active_plugins,
            original_phantom_id,
        }) => Ok(OplogEntry::Create {
            timestamp,
            worker_id,
            component_revision,
            env: env.into_iter().collect(),
            environment_id,
            created_by,
            config_vars,
            parent,
            component_size,
            initial_total_linear_memory_size,
            initial_active_plugins: initial_active_plugins
                .into_iter()
                .map(|x| x.plugin_priority)
                .collect(),
            original_phantom_id,
        }),
        PublicOplogEntry::HostCall(HostCallParams {
            timestamp,
            function_name,
            response,
            durable_function_type: wrapped_function_type,
            request,
        }) => {
            let durable_function_type = match wrapped_function_type {
                PublicDurableFunctionType::ReadLocal(_) => DurableFunctionType::ReadLocal,
                PublicDurableFunctionType::WriteLocal(_) => DurableFunctionType::WriteLocal,
                PublicDurableFunctionType::ReadRemote(_) => DurableFunctionType::ReadRemote,
                PublicDurableFunctionType::WriteRemote(_) => DurableFunctionType::WriteRemote,
                PublicDurableFunctionType::WriteRemoteBatched(params) => {
                    DurableFunctionType::WriteRemoteBatched(params.index)
                }
                PublicDurableFunctionType::WriteRemoteTransaction(params) => {
                    DurableFunctionType::WriteRemoteTransaction(params.index)
                }
            };

            let request = OplogPayload::Inline(Box::new(host_request_from_value_and_type(
                &function_name,
                request,
            )?));
            let response = OplogPayload::Inline(Box::new(host_response_from_value_and_type(
                &function_name,
                response,
            )?));

            Ok(OplogEntry::HostCall {
                timestamp,
                function_name: HostFunctionName::from(function_name.as_str()),
                request,
                response,
                durable_function_type,
            })
        }
        PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams { .. }) => {
            // TODO: Converting PublicAgentInvocation back to raw AgentInvocationPayload is not yet implemented
            Err("Converting AgentInvocationStarted from public to raw oplog entry is not yet supported".to_string())
        }

        PublicOplogEntry::Suspend(timestamp_parameter) => Ok(OplogEntry::Suspend {
            timestamp: timestamp_parameter.timestamp,
        }),
        PublicOplogEntry::Error(error) => Ok(OplogEntry::Error {
            timestamp: error.timestamp,
            error: WorkerError::Unknown(error.error),
            retry_from: error.retry_from,
        }),
        PublicOplogEntry::NoOp(timestamp_parameter) => Ok(OplogEntry::NoOp {
            timestamp: timestamp_parameter.timestamp,
        }),
        PublicOplogEntry::Jump(jump) => Ok(OplogEntry::Jump {
            timestamp: jump.timestamp,
            jump: jump.jump,
        }),
        PublicOplogEntry::Interrupted(interrupted) => Ok(OplogEntry::Interrupted {
            timestamp: interrupted.timestamp,
        }),
        PublicOplogEntry::Exited(exited) => Ok(OplogEntry::Exited {
            timestamp: exited.timestamp,
        }),
        PublicOplogEntry::ChangeRetryPolicy(change_retry_policy) => {
            Ok(OplogEntry::ChangeRetryPolicy {
                timestamp: change_retry_policy.timestamp,
                new_policy: RetryConfig {
                    max_attempts: change_retry_policy.new_policy.max_attempts,
                    min_delay: change_retry_policy.new_policy.min_delay,
                    max_delay: change_retry_policy.new_policy.max_delay,
                    multiplier: change_retry_policy.new_policy.multiplier,
                    max_jitter_factor: change_retry_policy.new_policy.max_jitter_factor,
                },
            })
        }
        PublicOplogEntry::BeginAtomicRegion(_) => {
            Err("Cannot override an oplog with a begin atomic region oplog".to_string())
        }
        PublicOplogEntry::EndAtomicRegion(_) => {
            Err("Cannot override an oplog with a end atomic region oplog".to_string())
        }
        PublicOplogEntry::BeginRemoteWrite(_) => {
            Err("Cannot override an oplog with a begin atomic write oplog".to_string())
        }
        PublicOplogEntry::EndRemoteWrite(_) => {
            Err("Cannot override an oplog with an end atomic write oplog".to_string())
        }
        PublicOplogEntry::PendingAgentInvocation(_) => {
            Err("Cannot override an oplog with a pending worker invocation".to_string())?
        }
        PublicOplogEntry::PendingUpdate(_) => {
            Err("Cannot override an oplog with a pending update".to_string())?
        }
        PublicOplogEntry::BeginRemoteTransaction(_) => {
            Err("Cannot override an oplog with a begin remote transaction".to_string())?
        }
        PublicOplogEntry::PreCommitRemoteTransaction(_) => {
            Err("Cannot override an oplog with a pre commit remote transaction".to_string())?
        }
        PublicOplogEntry::CommittedRemoteTransaction(_) => {
            Err("Cannot override an oplog with a committed remote transaction".to_string())?
        }
        PublicOplogEntry::PreRollbackRemoteTransaction(_) => {
            Err("Cannot override an oplog with a pre rollback remote transaction".to_string())?
        }
        PublicOplogEntry::RolledBackRemoteTransaction(_) => {
            Err("Cannot override an oplog with a rolled back remote transaction".to_string())?
        }
        PublicOplogEntry::SuccessfulUpdate(successful_update_params) => {
            let new_active_plugins: HashSet<PluginPriority> = successful_update_params
                .new_active_plugins
                .iter()
                .map(|plugin| plugin.plugin_priority)
                .collect();

            Ok(OplogEntry::SuccessfulUpdate {
                timestamp: successful_update_params.timestamp,
                target_revision: successful_update_params.target_revision,
                new_component_size: successful_update_params.new_component_size,
                new_active_plugins,
            })
        }
        PublicOplogEntry::FailedUpdate(FailedUpdateParams {
            timestamp,
            target_revision,
            details,
        }) => Ok(OplogEntry::FailedUpdate {
            timestamp,
            target_revision,
            details,
        }),
        PublicOplogEntry::GrowMemory(GrowMemoryParams { timestamp, delta }) => {
            Ok(OplogEntry::GrowMemory { timestamp, delta })
        }
        PublicOplogEntry::CreateResource(CreateResourceParams {
            timestamp,
            id,
            owner,
            name,
        }) => Ok(OplogEntry::CreateResource {
            timestamp,
            id,
            resource_type_id: ResourceTypeId { owner, name },
        }),
        PublicOplogEntry::DropResource(DropResourceParams {
            timestamp,
            id,
            owner,
            name,
        }) => Ok(OplogEntry::DropResource {
            timestamp,
            id,
            resource_type_id: ResourceTypeId { owner, name },
        }),
        PublicOplogEntry::Log(LogParams {
            timestamp,
            level,
            context,
            message,
        }) => Ok(OplogEntry::Log {
            timestamp,
            level,
            context,
            message,
        }),
        PublicOplogEntry::Restart(timestamp_parameter) => Ok(OplogEntry::Restart {
            timestamp: timestamp_parameter.timestamp,
        }),
        PublicOplogEntry::ActivatePlugin(activate_plugin_params) => {
            Ok(OplogEntry::ActivatePlugin {
                timestamp: activate_plugin_params.timestamp,
                plugin_priority: activate_plugin_params.plugin.plugin_priority,
            })
        }
        PublicOplogEntry::DeactivatePlugin(deactivate_plugin_params) => {
            Ok(OplogEntry::DeactivatePlugin {
                timestamp: deactivate_plugin_params.timestamp,
                plugin_priority: deactivate_plugin_params.plugin.plugin_priority,
            })
        }
        PublicOplogEntry::Revert(revert_params) => Ok(OplogEntry::Revert {
            timestamp: revert_params.timestamp,
            dropped_region: revert_params.dropped_region,
        }),
        PublicOplogEntry::CancelPendingInvocation(cancel_invocation_params) => {
            Ok(OplogEntry::CancelPendingInvocation {
                timestamp: cancel_invocation_params.timestamp,
                idempotency_key: cancel_invocation_params.idempotency_key,
            })
        }
        PublicOplogEntry::StartSpan(start_span) => Ok(OplogEntry::StartSpan {
            timestamp: start_span.timestamp,
            span_id: start_span.span_id,
            parent: start_span.parent_id,
            linked_context_id: start_span.linked_context,
            attributes: start_span
                .attributes
                .into_iter()
                .map(|attr| (attr.key, attr.value.into()))
                .collect::<HashMap<_, _>>()
                .into(),
        }),
        PublicOplogEntry::FinishSpan(finish_span) => Ok(OplogEntry::FinishSpan {
            timestamp: finish_span.timestamp,
            span_id: finish_span.span_id,
        }),
        PublicOplogEntry::SetSpanAttribute(set_span_attribute) => {
            Ok(OplogEntry::SetSpanAttribute {
                timestamp: set_span_attribute.timestamp,
                span_id: set_span_attribute.span_id,
                key: set_span_attribute.key,
                value: set_span_attribute.value.into(),
            })
        }
        PublicOplogEntry::ChangePersistenceLevel(change_persistence_level) => {
            Ok(OplogEntry::ChangePersistenceLevel {
                timestamp: change_persistence_level.timestamp,
                persistence_level: change_persistence_level.persistence_level,
            })
        }
        PublicOplogEntry::Snapshot(snapshot_params) => {
            let bytes = match snapshot_params.data {
                PublicSnapshotData::Raw(raw) => (raw.data, raw.mime_type),
                PublicSnapshotData::Json(json) => (
                    serde_json::to_vec(&json.data).map_err(|e| e.to_string())?,
                    "application/json".to_string(),
                ),
            };
            Ok(OplogEntry::Snapshot {
                timestamp: snapshot_params.timestamp,
                data: OplogPayload::Inline(Box::new(bytes.0)),
                mime_type: bytes.1,
            })
        }
    }
}

fn public_agent_invocation_result_to_raw(
    result: PublicAgentInvocationResult,
) -> Result<AgentInvocationResult, String> {
    match result {
        PublicAgentInvocationResult::AgentInitialization(_) => {
            Ok(AgentInvocationResult::AgentInitialization)
        }
        PublicAgentInvocationResult::AgentMethod(_) => Ok(AgentInvocationResult::AgentMethod {
            output: UntypedDataValue::Tuple(vec![]),
        }),
        PublicAgentInvocationResult::ManualUpdate(_) => Ok(AgentInvocationResult::ManualUpdate),
        PublicAgentInvocationResult::LoadSnapshot(params) => {
            Ok(AgentInvocationResult::LoadSnapshot {
                error: params.error,
            })
        }
        PublicAgentInvocationResult::SaveSnapshot(params) => {
            let snapshot = match params.snapshot {
                PublicSnapshotData::Raw(raw) => raw,
                PublicSnapshotData::Json(json) => RawSnapshotData {
                    data: serde_json::to_vec(&json.data).map_err(|e| e.to_string())?,
                    mime_type: "application/json".to_string(),
                },
            };
            Ok(AgentInvocationResult::SaveSnapshot { snapshot })
        }
        PublicAgentInvocationResult::ProcessOplogEntries(params) => {
            Ok(AgentInvocationResult::ProcessOplogEntries {
                error: params.error,
            })
        }
    }
}
