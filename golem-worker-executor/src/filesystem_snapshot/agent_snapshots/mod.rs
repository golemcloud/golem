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

//! The snapshot namespace of one agent incarnation.

use golem_common::model::{AgentFingerprint, OwnedAgentId};
use golem_service_base::storage::blob::BlobStorageNamespace;

/// The filesystem snapshots of one incarnation of an agent.
///
/// Each incarnation has one value: the agent with the fingerprint of the incarnation. A new
/// incarnation of the same agent id has another fingerprint, so it never shares the snapshots of
/// an earlier one. The value is opaque outside the filesystem snapshot store. Many owners hold it,
/// so a clone shares the namespace instead of copying it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AgentSnapshots(
    pub(in crate::filesystem_snapshot) std::sync::Arc<BlobStorageNamespace>,
);

impl AgentSnapshots {
    /// Gives the filesystem snapshots of the incarnation `fingerprint` of the agent.
    pub(crate) fn agent(agent: &OwnedAgentId, fingerprint: AgentFingerprint) -> Self {
        Self(std::sync::Arc::new(
            BlobStorageNamespace::FilesystemSnapshots {
                environment_id: agent.environment_id,
                agent_id: agent.agent_id.clone(),
                fingerprint,
            },
        ))
    }
}
