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

use crate::model::oplog::payload::{
    HostRequestGolemApiOplogRead, HostResponseGolemApiAgentId, HostResponseGolemApiAgentMetadata,
    HostResponseGolemApiFork, HostResponseGolemApiOplogChunk,
};

super::denied_with_message! {
    unit => HostResponseGolemApiUnit,
    self_metadata => HostResponseGolemApiSelfAgentMetadata,
    agents => HostResponseGolemApiAgents,
    oplog_entries => HostResponseGolemApiOplogEntries,
    webhook_url => HostResponseGolemAgentWebhookUrl,
    config_get => HostResponseConfigGetResponse,
}

/// A denied strict agent id resolution reports that the agent is not found.
pub fn agent_id_not_found<R>(_request: &R, _denial: &str) -> HostResponseGolemApiAgentId {
    HostResponseGolemApiAgentId { result: Ok(None) }
}

/// A denied agent metadata lookup reports that the agent does not exist.
pub fn agent_metadata<R>(_request: &R, _denial: &str) -> HostResponseGolemApiAgentMetadata {
    HostResponseGolemApiAgentMetadata { result: Ok(None) }
}

pub fn fork<R>(_request: &R, denial: &str) -> HostResponseGolemApiFork {
    HostResponseGolemApiFork {
        forked_phantom_id: uuid::Uuid::nil(),
        result: Err(denial.to_string()),
    }
}

/// The resource state of a denied oplog read does not advance; the caller keeps its own state.
pub fn oplog_chunk(
    request: &HostRequestGolemApiOplogRead,
    denial: &str,
) -> HostResponseGolemApiOplogChunk {
    HostResponseGolemApiOplogChunk {
        result: Err(denial.to_string()),
        next_oplog_index: request.next_oplog_index,
        current_component_revision: request
            .current_component_revision
            .unwrap_or(crate::model::component::ComponentRevision::INITIAL),
    }
}
