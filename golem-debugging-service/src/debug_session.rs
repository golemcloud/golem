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

use crate::model::params::PlaybackOverride;
use async_trait::async_trait;
use golem_common::model::oplog::PublicOplogEntry;
use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequest, HostStreamKind, OplogEntry, OplogIndex,
};
use golem_common::model::{AgentId, AgentMetadata, OwnedAgentId};
use golem_worker_executor::services::oplog::{Oplog, OplogOps};
use serde::Serialize;
use std::collections::HashMap;
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
    pub worker_metadata: AgentMetadata,
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
            let oplog_entry = OplogEntry::try_from(public_oplog_entry).map_err(|reason| {
                format!("Cannot use oplog entry as a playback override: {reason}")
            })?;
            overrides.insert(oplog_index, oplog_entry);
        }
        Ok(Self { overrides })
    }
}

/// The replay identity an oplog entry carries in the paired durable constructs (durable call
/// `Start`/`End`/`Cancelled`, atomic regions, remote transactions, host stream frames). Replay
/// resolves these pairs by index and claims `Start` entries by their full identity — function
/// name, durable function type, parent index, request presence, and (for concurrent sibling
/// calls) the request value itself — so a playback override must preserve the underlying entry's
/// signature or replay either fails to find its own record or claims a sibling call's record.
#[derive(Debug, Clone, PartialEq)]
enum PairingSignature {
    Start {
        function_name: HostFunctionName,
        durable_function_type: DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
        /// The resolved request value (not the raw payload bytes): replay's request-matched
        /// claims compare deserialized [`HostRequest`] values, and payload serialization is not
        /// canonical, so the comparison must be semantic.
        request: Option<Box<HostRequest>>,
    },
    End {
        start_index: OplogIndex,
    },
    Cancelled {
        start_index: OplogIndex,
    },
    HostStreamFrame {
        parent_start_index: OplogIndex,
        kind: HostStreamKind,
    },
    BeginAtomicRegion,
    EndAtomicRegion {
        begin_index: OplogIndex,
    },
    BeginRemoteTransaction,
    PreCommitRemoteTransaction {
        begin_index: OplogIndex,
    },
    PreRollbackRemoteTransaction {
        begin_index: OplogIndex,
    },
    CommittedRemoteTransaction {
        begin_index: OplogIndex,
    },
    RolledBackRemoteTransaction {
        begin_index: OplogIndex,
    },
    Unpaired,
}

impl PairingSignature {
    /// Computes the entry's replay identity. `oplog` is used to resolve a `Start` entry's request
    /// payload (which may live in external storage) into its [`HostRequest`] value.
    async fn of(entry: &OplogEntry, oplog: &Arc<dyn Oplog>) -> Result<Self, String> {
        Ok(match entry {
            OplogEntry::Start {
                function_name,
                request,
                durable_function_type,
                parent_start_index,
                ..
            } => {
                let request = match request {
                    Some(payload) => Some(Box::new(
                        oplog.download_payload(payload.clone()).await.map_err(|err| {
                            format!(
                                "Failed to resolve the request payload of the Start entry: {err}"
                            )
                        })?,
                    )),
                    None => None,
                };
                PairingSignature::Start {
                    function_name: function_name.clone(),
                    durable_function_type: durable_function_type.clone(),
                    parent_start_index: *parent_start_index,
                    request,
                }
            }
            OplogEntry::End { start_index, .. } => PairingSignature::End {
                start_index: *start_index,
            },
            OplogEntry::Cancelled { start_index, .. } => PairingSignature::Cancelled {
                start_index: *start_index,
            },
            OplogEntry::HostStreamFrame {
                parent_start_index,
                kind,
                ..
            } => PairingSignature::HostStreamFrame {
                parent_start_index: *parent_start_index,
                kind: *kind,
            },
            OplogEntry::BeginAtomicRegion { .. } => PairingSignature::BeginAtomicRegion,
            OplogEntry::EndAtomicRegion { begin_index, .. } => PairingSignature::EndAtomicRegion {
                begin_index: *begin_index,
            },
            OplogEntry::BeginRemoteTransaction { .. } => PairingSignature::BeginRemoteTransaction,
            OplogEntry::PreCommitRemoteTransaction { begin_index, .. } => {
                PairingSignature::PreCommitRemoteTransaction {
                    begin_index: *begin_index,
                }
            }
            OplogEntry::PreRollbackRemoteTransaction { begin_index, .. } => {
                PairingSignature::PreRollbackRemoteTransaction {
                    begin_index: *begin_index,
                }
            }
            OplogEntry::CommittedRemoteTransaction { begin_index, .. } => {
                PairingSignature::CommittedRemoteTransaction {
                    begin_index: *begin_index,
                }
            }
            OplogEntry::RolledBackRemoteTransaction { begin_index, .. } => {
                PairingSignature::RolledBackRemoteTransaction {
                    begin_index: *begin_index,
                }
            }
            _ => PairingSignature::Unpaired,
        })
    }

    /// A short human-readable form for validation errors, eliding the potentially large request
    /// value.
    fn describe(&self) -> String {
        match self {
            PairingSignature::Start {
                function_name,
                durable_function_type,
                parent_start_index,
                request,
            } => format!(
                "Start {{ function_name: {function_name}, durable_function_type: {durable_function_type:?}, parent_start_index: {parent_start_index:?}, request: {} }}",
                if request.is_some() {
                    "Some(..)"
                } else {
                    "None"
                }
            ),
            other => format!("{other:?}"),
        }
    }
}

/// Checks that a playback override entry preserves the replay identity of the oplog entry it
/// replaces: a `Start` must stay a `Start` with the same function name, durable function type,
/// parent and request value; an `End`/`Cancelled` must keep referencing the same `Start` index;
/// remote transaction stage entries must keep their stage; and so on. Otherwise the override
/// would corrupt the replay resolver's `Start`/terminal matching, or make a replayed call claim
/// a concurrent sibling call's record.
pub async fn validate_override_preserves_pairing(
    index: OplogIndex,
    underlying: &OplogEntry,
    override_entry: &OplogEntry,
    oplog: &Arc<dyn Oplog>,
) -> Result<(), String> {
    let underlying_signature = PairingSignature::of(underlying, oplog)
        .await
        .map_err(|err| {
            format!("Cannot validate the playback override at oplog index {index}: {err}")
        })?;
    let override_signature = PairingSignature::of(override_entry, oplog)
        .await
        .map_err(|err| {
            format!("Cannot validate the playback override at oplog index {index}: {err}")
        })?;
    if underlying_signature == override_signature {
        Ok(())
    } else {
        let detail = if let (
            PairingSignature::Start {
                request: Some(underlying_request),
                ..
            },
            PairingSignature::Start {
                request: Some(override_request),
                ..
            },
        ) = (&underlying_signature, &override_signature)
            && underlying_request != override_request
        {
            " (the request values differ)"
        } else {
            ""
        };
        Err(format!(
            "Playback override at oplog index {index} would break durable entry pairing: the recorded entry is {} but the override is {}{detail}",
            underlying_signature.describe(),
            override_signature.describe(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DebugSessionId(AgentId);

impl Serialize for DebugSessionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Serialize::serialize(&self.0.to_string(), serializer)
    }
}

impl DebugSessionId {
    pub fn new(agent_id: OwnedAgentId) -> Self {
        DebugSessionId(agent_id.agent_id)
    }

    pub fn agent_id(&self) -> AgentId {
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
    pub agent_id: AgentId,
}

impl ActiveSessionData {
    pub fn new(agent_id: AgentId) -> Self {
        Self { agent_id }
    }
}
