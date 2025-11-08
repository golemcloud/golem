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
pub mod component;
pub mod plugin_registration;

use golem_common::model::component::{ComponentFilePermissions, ComponentRevision};
use golem_common::model::public_oplog::{OplogCursor, PublicOplogEntryWithIndex};
use golem_common::model::worker::{
    FlatComponentFileSystemNode, FlatComponentFileSystemNodeKind, WorkerUpdateMode,
};
use golem_common::model::{OplogIndex, ScanCursor, WorkerFilter, WorkerId};
use golem_wasm::ValueAndType;
use golem_wasm::json::OptionallyValueAndTypeJson;
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerCreationResponse {
    pub worker_id: WorkerId,
    pub component_revision: ComponentRevision,
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
pub struct ForkWorkerResponse {}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct UpdateWorkerRequest {
    pub mode: WorkerUpdateMode,
    pub target_revision: ComponentRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ForkWorkerRequest {
    pub target_worker_id: WorkerId,
    pub oplog_index_cutoff: OplogIndex,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct InvokeResult {
    pub result: Option<ValueAndType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ResourceLimits {
    pub available_fuel: i64,
    pub max_memory_per_worker: u64,
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

#[derive(Clone, Debug, PartialEq)]
pub enum GetFileSystemNodeResult {
    Ok(Vec<ComponentFileSystemNode>),
    File(ComponentFileSystemNode),
    NotFound,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ComponentFileSystemNodeDetails {
    File {
        permissions: ComponentFilePermissions,
        size: u64,
    },
    Directory,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComponentFileSystemNode {
    pub name: String,
    pub last_modified: SystemTime,
    pub details: ComponentFileSystemNodeDetails,
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

impl From<ComponentFileSystemNode> for golem_api_grpc::proto::golem::worker::FileSystemNode {
    fn from(value: ComponentFileSystemNode) -> Self {
        let last_modified = value
            .last_modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        match value.details {
            ComponentFileSystemNodeDetails::File { permissions, size } =>
                golem_api_grpc::proto::golem::worker::FileSystemNode {
                    value: Some(golem_api_grpc::proto::golem::worker::file_system_node::Value::File(
                        golem_api_grpc::proto::golem::worker::FileFileSystemNode {
                            name: value.name,
                            last_modified,
                            size,
                            permissions:
                            golem_api_grpc::proto::golem::component::ComponentFilePermissions::from(permissions).into(),
                        }
                    ))
                },
            ComponentFileSystemNodeDetails::Directory =>
                golem_api_grpc::proto::golem::worker::FileSystemNode {
                    value: Some(golem_api_grpc::proto::golem::worker::file_system_node::Value::Directory(
                        golem_api_grpc::proto::golem::worker::DirectoryFileSystemNode {
                            name: value.name,
                            last_modified,
                        }
                    ))
                }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::FileSystemNode> for ComponentFileSystemNode {
    type Error = anyhow::Error;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::FileSystemNode,
    ) -> Result<Self, Self::Error> {
        match value.value {
            Some(golem_api_grpc::proto::golem::worker::file_system_node::Value::Directory(
                golem_api_grpc::proto::golem::worker::DirectoryFileSystemNode {
                    name,
                    last_modified,
                },
            )) => Ok(ComponentFileSystemNode {
                name,
                last_modified: SystemTime::UNIX_EPOCH + Duration::from_secs(last_modified),
                details: ComponentFileSystemNodeDetails::Directory,
            }),
            Some(golem_api_grpc::proto::golem::worker::file_system_node::Value::File(
                golem_api_grpc::proto::golem::worker::FileFileSystemNode {
                    name,
                    last_modified,
                    size,
                    permissions,
                },
            )) => Ok(ComponentFileSystemNode {
                name,
                last_modified: SystemTime::UNIX_EPOCH + Duration::from_secs(last_modified),
                details: ComponentFileSystemNodeDetails::File {
                    permissions:
                        golem_api_grpc::proto::golem::component::ComponentFilePermissions::try_from(
                            permissions,
                        )?
                        .into(),
                    size,
                },
            }),
            None => Err(anyhow::anyhow!("Missing value")),
        }
    }
}
