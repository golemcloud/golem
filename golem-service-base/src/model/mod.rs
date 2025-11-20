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

pub mod auth;

use applying::Apply;
use golem_api_grpc::proto::golem::worker::OplogEntryWithIndex;
use golem_common::model::component::{ComponentOwner, VersionedComponentId};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::oplog::{OplogCursor, PublicOplogEntry};
use golem_common::model::plugin::{PluginInstallation, PluginInstallationAction};
use golem_common::model::{
    ComponentFilePermissions, ComponentFileSystemNode, ComponentFileSystemNodeDetails,
    ComponentVersion, InitialComponentFile, ScanCursor, Timestamp, WorkerFilter, WorkerId,
};
use golem_wasm::json::OptionallyValueAndTypeJson;
use golem_wasm::ValueAndType;
use poem_openapi::{Enum, NewType, Object, Union};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use std::{collections::HashMap, fmt::Display, fmt::Formatter};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerCreationResponse {
    pub worker_id: WorkerId,
    pub component_version: ComponentVersion,
}

// TODO: Add validations (non-empty, no "/", no " ", ...)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, NewType)]
pub struct ComponentName(pub String);

impl Display for ComponentName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CompleteParameters {
    pub oplog_idx: u64,
    pub data: Vec<u8>,
}

impl From<CompleteParameters> for golem_api_grpc::proto::golem::worker::CompleteParameters {
    fn from(value: CompleteParameters) -> Self {
        Self {
            oplog_idx: value.oplog_idx,
            data: value.data,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct InvokeParameters {
    pub params: Vec<OptionallyValueAndTypeJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct DeleteWorkerResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct InvokeResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct InterruptResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct ResumeResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct UpdateWorkerResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct ActivatePluginResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct DeactivatePluginResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct RevertWorkerResponse {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
pub struct CancelInvocationResponse {
    pub canceled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GetOplogResponse {
    pub entries: Vec<PublicOplogEntryWithIndex>,
    pub next: Option<OplogCursor>,
    pub first_index_in_chunk: u64,
    pub last_index: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GetFilesResponse {
    pub nodes: Vec<FlatComponentFileSystemNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Enum)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub enum FlatComponentFileSystemNodeKind {
    Directory,
    File,
}

// Flat, worse typed version ComponentFileSystemNode for rest representation
#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct FlatComponentFileSystemNode {
    pub name: String,
    pub last_modified: u64,
    pub kind: FlatComponentFileSystemNodeKind,
    pub permissions: Option<ComponentFilePermissions>, // only for files
    pub size: Option<u64>,                             // only for files
}

impl From<ComponentFileSystemNode> for FlatComponentFileSystemNode {
    fn from(value: ComponentFileSystemNode) -> Self {
        let last_modified = value
            .last_modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        match value.details {
            ComponentFileSystemNodeDetails::Directory => Self {
                name: value.name,
                last_modified,
                kind: FlatComponentFileSystemNodeKind::Directory,
                permissions: None,
                size: None,
            },
            ComponentFileSystemNodeDetails::File { permissions, size } => Self {
                name: value.name,
                last_modified,
                kind: FlatComponentFileSystemNodeKind::File,
                permissions: Some(permissions),
                size: Some(size),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PublicOplogEntryWithIndex {
    pub oplog_index: OplogIndex,
    pub entry: PublicOplogEntry,
}

impl TryFrom<OplogEntryWithIndex> for PublicOplogEntryWithIndex {
    type Error = String;

    fn try_from(value: OplogEntryWithIndex) -> Result<Self, Self::Error> {
        Ok(Self {
            oplog_index: OplogIndex::from_u64(value.oplog_index),
            entry: value.entry.ok_or("Missing field: entry")?.try_into()?,
        })
    }
}

impl TryFrom<PublicOplogEntryWithIndex> for OplogEntryWithIndex {
    type Error = String;

    fn try_from(value: PublicOplogEntryWithIndex) -> Result<Self, Self::Error> {
        Ok(Self {
            oplog_index: value.oplog_index.into(),
            entry: Some(value.entry.try_into()?),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Enum)]
pub enum WorkerUpdateMode {
    Automatic,
    Manual,
}

impl From<golem_api_grpc::proto::golem::worker::UpdateMode> for WorkerUpdateMode {
    fn from(value: golem_api_grpc::proto::golem::worker::UpdateMode) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::UpdateMode::Automatic => {
                WorkerUpdateMode::Automatic
            }
            golem_api_grpc::proto::golem::worker::UpdateMode::Manual => WorkerUpdateMode::Manual,
        }
    }
}

impl From<WorkerUpdateMode> for golem_api_grpc::proto::golem::worker::UpdateMode {
    fn from(value: WorkerUpdateMode) -> Self {
        match value {
            WorkerUpdateMode::Automatic => {
                golem_api_grpc::proto::golem::worker::UpdateMode::Automatic
            }
            WorkerUpdateMode::Manual => golem_api_grpc::proto::golem::worker::UpdateMode::Manual,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct UpdateWorkerRequest {
    pub mode: WorkerUpdateMode,
    pub target_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct WorkersMetadataRequest {
    pub filter: Option<WorkerFilter>,
    pub cursor: Option<ScanCursor>,
    pub count: Option<u64>,
    pub precise: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Union)]
#[serde(rename_all = "camelCase")]
#[oai(discriminator_name = "type", one_of = true, rename_all = "camelCase")]
pub enum UpdateRecord {
    PendingUpdate(PendingUpdate),
    SuccessfulUpdate(SuccessfulUpdate),
    FailedUpdate(FailedUpdate),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PendingUpdate {
    timestamp: Timestamp,
    target_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct SuccessfulUpdate {
    timestamp: Timestamp,
    target_version: ComponentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct FailedUpdate {
    timestamp: Timestamp,
    target_version: ComponentVersion,
    details: Option<String>,
}

impl TryFrom<golem_api_grpc::proto::golem::worker::UpdateRecord> for UpdateRecord {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::UpdateRecord,
    ) -> Result<Self, Self::Error> {
        match value.update.ok_or("Missing update field")? {
            golem_api_grpc::proto::golem::worker::update_record::Update::Failed(failed) => {
                Ok(Self::FailedUpdate(FailedUpdate {
                    timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                    target_version: value.target_version,
                    details: { failed.details },
                }))
            }
            golem_api_grpc::proto::golem::worker::update_record::Update::Pending(_) => {
                Ok(Self::PendingUpdate(PendingUpdate {
                    timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                    target_version: value.target_version,
                }))
            }
            golem_api_grpc::proto::golem::worker::update_record::Update::Successful(_) => {
                Ok(Self::SuccessfulUpdate(SuccessfulUpdate {
                    timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                    target_version: value.target_version,
                }))
            }
        }
    }
}

impl From<UpdateRecord> for golem_api_grpc::proto::golem::worker::UpdateRecord {
    fn from(value: UpdateRecord) -> Self {
        match value {
            UpdateRecord::FailedUpdate(FailedUpdate {
                timestamp,
                target_version,
                details,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_version,
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Failed(
                        golem_api_grpc::proto::golem::worker::FailedUpdate { details },
                    ),
                ),
            },
            UpdateRecord::PendingUpdate(PendingUpdate {
                timestamp,
                target_version,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_version,
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Pending(
                        golem_api_grpc::proto::golem::worker::PendingUpdate {},
                    ),
                ),
            },
            UpdateRecord::SuccessfulUpdate(SuccessfulUpdate {
                timestamp,
                target_version,
            }) => Self {
                timestamp: Some(timestamp.into()),
                target_version,
                update: Some(
                    golem_api_grpc::proto::golem::worker::update_record::Update::Successful(
                        golem_api_grpc::proto::golem::worker::SuccessfulUpdate {},
                    ),
                ),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct InvokeResult {
    pub result: Option<ValueAndType>,
}

#[derive(Debug, Clone)]
pub struct Component {
    pub owner: ComponentOwner,
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub files: Vec<InitialComponentFile>,
    pub installed_plugins: Vec<PluginInstallation>,
    pub env: HashMap<String, String>,
}

impl TryFrom<golem_api_grpc::proto::golem::component::Component> for Component {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Component,
    ) -> Result<Self, Self::Error> {
        let account_id = value.account_id.ok_or("missing account_id")?.into();

        let project_id = value
            .project_id
            .ok_or("missing project_id")?
            .try_into()
            .map_err(|_| "Failed to convert project_id".to_string())?;

        let created_at = value
            .created_at
            .ok_or("missing created_at")?
            .apply(SystemTime::try_from)
            .map_err(|_| "Failed to convert timestamp".to_string())?
            .into();

        let files = value
            .files
            .into_iter()
            .map(|f| f.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        let installed_plugins = value
            .installed_plugins
            .into_iter()
            .map(|p| p.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            owner: ComponentOwner {
                account_id,
                project_id,
            },
            versioned_component_id: value
                .versioned_component_id
                .ok_or("Missing versioned_component_id")?
                .try_into()?,
            component_name: ComponentName(value.component_name.clone()),
            component_size: value.component_size,
            metadata: value
                .metadata
                .clone()
                .ok_or("Missing metadata")?
                .try_into()?,
            created_at,
            files,
            installed_plugins,
            env: value.env,
        })
    }
}

impl From<Component> for golem_api_grpc::proto::golem::component::Component {
    fn from(value: Component) -> Self {
        Self {
            account_id: Some(value.owner.account_id.into()),
            project_id: Some(value.owner.project_id.into()),
            versioned_component_id: Some(value.versioned_component_id.into()),
            component_name: value.component_name.0,
            component_size: value.component_size,
            metadata: Some(value.metadata.into()),
            created_at: Some(SystemTime::from(value.created_at).into()),
            files: value.files.into_iter().map(Into::into).collect(),
            installed_plugins: value
                .installed_plugins
                .into_iter()
                .map(Into::into)
                .collect(),
            env: value.env,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ResourceLimits {
    pub available_fuel: i64,
    pub max_memory_per_worker: i64,
}

impl From<ResourceLimits> for golem_api_grpc::proto::golem::common::ResourceLimits {
    fn from(value: ResourceLimits) -> Self {
        Self {
            available_fuel: value.available_fuel,
            max_memory_per_worker: value.max_memory_per_worker,
        }
    }
}

impl From<golem_api_grpc::proto::golem::common::ResourceLimits> for ResourceLimits {
    fn from(value: golem_api_grpc::proto::golem::common::ResourceLimits) -> Self {
        Self {
            available_fuel: value.available_fuel,
            max_memory_per_worker: value.max_memory_per_worker,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct BatchPluginInstallationUpdates {
    pub actions: Vec<PluginInstallationAction>,
}
