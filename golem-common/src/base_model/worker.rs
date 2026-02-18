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

use crate::base_model::account::AccountId;
use crate::base_model::component::{ComponentFilePermissions, ComponentRevision, PluginPriority};
use crate::base_model::environment::EnvironmentId;
use crate::base_model::oplog::WorkerResourceId;
use crate::base_model::regions::OplogRegion;
use crate::base_model::{OplogIndex, Timestamp, WorkerId, WorkerResourceDescription, WorkerStatus};
use crate::{declare_enums, declare_structs, declare_unions};
use golem_wasm_derive::{FromValue, IntoValue};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
pub struct WorkerCreationRequest {
    pub name: String,
    pub env: HashMap<String, String>,
    #[cfg_attr(feature = "full", oai(default))]
    pub config_vars: BTreeMap<String, String>,
}

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

declare_structs! {
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
        pub key: WorkerResourceId,
        pub description: WorkerResourceDescription,
    }

    pub struct WorkerMetadataDto {
        pub worker_id: WorkerId,
        pub environment_id: EnvironmentId,
        pub created_by: AccountId,
        pub env: HashMap<String, String>,
        pub config_vars: BTreeMap<String, String>,
        pub status: WorkerStatus,
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

    pub struct FlatComponentFileSystemNode {
        pub name: String,
        pub last_modified: u64,
        pub kind: FlatComponentFileSystemNodeKind,
        pub permissions: Option<ComponentFilePermissions>, // only for files
        pub size: Option<u64>,                             // only for files
    }
}

declare_enums! {
    pub enum WorkerUpdateMode {
        Automatic,
        Manual,
    }
}
