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
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AgentId, AgentMetadata, OwnedAgentId};
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
