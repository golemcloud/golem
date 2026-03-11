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

use crate::base_model::account::AccountId;
use crate::base_model::component::{ComponentFilePermissions, ComponentRevision, PluginPriority};
use crate::base_model::environment::EnvironmentId;
use crate::base_model::oplog::AgentResourceId;
use crate::base_model::regions::OplogRegion;
use crate::base_model::{AgentId, AgentResourceDescription, AgentStatus, OplogIndex, Timestamp};
use crate::{declare_enums, declare_structs, declare_unions};
use golem_wasm_derive::{FromValue, IntoValue};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{Display, Formatter};

declare_enums! {
    pub enum FlatComponentFileSystemNodeKind {
        Directory,
        File,
    }
}

impl Display for FlatComponentFileSystemNodeKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            FlatComponentFileSystemNodeKind::Directory => write!(f, "directory"),
            FlatComponentFileSystemNodeKind::File => write!(f, "file"),
        }
    }
}

declare_unions! {
    pub enum UpdateRecord {
        PendingUpdate(PendingUpdate),
        SuccessfulUpdate(SuccessfulUpdate),
        FailedUpdate(FailedUpdate),
    }

    #[derive(IntoValue, FromValue)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub enum RevertWorkerTarget {
        RevertToOplogIndex(RevertToOplogIndex),
        RevertLastInvocations(RevertLastInvocations),
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "full",
    derive(IntoValue, FromValue, desert_rust::BinaryCodec)
)]
#[cfg_attr(
    feature = "full",
    wit(name = "raw-local-agent-config-entry", owner = "golem:api@1.5.0/oplog")
)]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct UntypedParsedWorkerCreationLocalAgentConfigEntry {
    pub key: Vec<String>,
    pub value: golem_wasm::Value,
}

declare_structs! {
    pub struct WorkerCreationLocalAgentConfigEntry {
        pub key: Vec<String>,
        pub value: serde_json::Value
    }

    #[cfg_attr(feature = "full", derive(IntoValue, FromValue, desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", wit(name = "local-agent-config-entry", owner = "golem:api@1.5.0/oplog"))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct ParsedWorkerCreationLocalAgentConfigEntry {
        pub key: Vec<String>,
        pub value: golem_wasm::ValueAndType
    }

    pub struct AgentCreationRequest {
        pub name: String,
        pub env: HashMap<String, String>,
        #[cfg_attr(feature = "full", oai(default))]
        pub config_vars: BTreeMap<String, String>,
        #[cfg_attr(feature = "full", oai(default))]
        pub local_agent_config: Vec<WorkerCreationLocalAgentConfigEntry>
    }

    pub struct PendingUpdate {
        pub timestamp: Timestamp,
        pub target_revision: ComponentRevision,
    }

    pub struct SuccessfulUpdate {
        pub timestamp: Timestamp,
        pub target_revision: ComponentRevision,
    }

    pub struct FailedUpdate {
        pub timestamp: Timestamp,
        pub target_revision: ComponentRevision,
        pub details: Option<String>,
    }

    pub struct ExportedResourceMetadata {
        pub key: AgentResourceId,
        pub description: AgentResourceDescription,
    }

    pub struct AgentMetadataDto {
        pub agent_id: AgentId,
        pub environment_id: EnvironmentId,
        pub created_by: AccountId,
        pub env: HashMap<String, String>,
        pub config_vars: BTreeMap<String, String>,
        pub status: AgentStatus,
        pub component_revision: ComponentRevision,
        pub retry_count: u32,
        pub pending_invocation_count: u64,
        pub updates: Vec<UpdateRecord>,
        pub created_at: Timestamp,
        pub last_error: Option<String>,
        pub component_size: u64,
        pub total_linear_memory_size: u64,
        pub exported_resource_instances: Vec<ExportedResourceMetadata>,
        pub active_plugins: HashSet<PluginPriority>,
        /// Oplog regions that are skipped during the worker's state recovery, but describe
        /// the history of the worker. For example if an atomic region gets restarted, its partially
        /// recorded oplog entries will be skipped on retry.
        pub skipped_regions: Vec<OplogRegion>,
        /// Oplog regions permanently deleted from the workers using the revert functionality.
        pub deleted_regions: Vec<OplogRegion>
    }

    #[derive(IntoValue, FromValue)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct RevertToOplogIndex {
        pub last_oplog_index: OplogIndex,
    }

    #[derive(IntoValue, FromValue)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct RevertLastInvocations {
        pub number_of_invocations: u64,
    }

    // TODO: Rename to AgentFileSystemNode
    pub struct FlatComponentFileSystemNode {
        pub name: String,
        pub last_modified: u64,
        pub kind: FlatComponentFileSystemNodeKind,
        pub permissions: Option<ComponentFilePermissions>, // only for files
        pub size: Option<u64>,                             // only for files
    }
}

declare_enums! {
    pub enum AgentUpdateMode {
        Automatic,
        Manual,
    }
}

impl From<ParsedWorkerCreationLocalAgentConfigEntry>
    for UntypedParsedWorkerCreationLocalAgentConfigEntry
{
    fn from(value: ParsedWorkerCreationLocalAgentConfigEntry) -> Self {
        Self {
            key: value.key,
            value: value.value.value,
        }
    }
}
