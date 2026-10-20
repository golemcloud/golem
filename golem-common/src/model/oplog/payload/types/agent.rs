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

use wasmtime_wasi::StreamError;

use super::{AgentMetadataForGuests, SerializableScheduleId, SerializableStreamError};
use crate::model::agent::TypedAgentConfigEntry;
use crate::model::{AgentMetadata, ScheduleId};

impl From<AgentMetadata> for AgentMetadataForGuests {
    fn from(value: AgentMetadata) -> Self {
        Self {
            agent_id: value.agent_id,
            args: vec![],
            env: value.env,
            config: TypedAgentConfigEntry::to_flat_map(&value.config),
            status: value.last_known_status.status,
            component_revision: value.last_known_status.component_revision,
            retry_count: value
                .last_known_status
                .current_retry_state
                .iter()
                .max_by_key(|(idx, _)| **idx)
                .map(|(_, state)| state.retry_count())
                .unwrap_or_default() as u64,
            environment_id: value.environment_id,
        }
    }
}

impl From<StreamError> for SerializableStreamError {
    fn from(value: StreamError) -> Self {
        Self::from(&value)
    }
}

impl From<&StreamError> for SerializableStreamError {
    fn from(value: &StreamError) -> Self {
        match value {
            StreamError::Closed => Self::Closed,
            StreamError::LastOperationFailed(e) => Self::LastOperationFailed(e.to_string()),
            StreamError::Trap(e) => Self::Trap(e.to_string()),
        }
    }
}

impl From<SerializableStreamError> for StreamError {
    fn from(value: SerializableStreamError) -> Self {
        match value {
            SerializableStreamError::Closed => Self::Closed,
            SerializableStreamError::LastOperationFailed(e) => {
                Self::LastOperationFailed(wasmtime::Error::msg(e))
            }
            SerializableStreamError::Trap(e) => Self::Trap(wasmtime::Error::msg(e)),
        }
    }
}

impl SerializableScheduleId {
    pub fn from_domain(schedule_id: ScheduleId) -> Self {
        Self { id: schedule_id.id }
    }

    pub fn into_domain(self) -> ScheduleId {
        ScheduleId { id: self.id }
    }
}
