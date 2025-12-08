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

use super::account::AccountId;
use super::component::ComponentFilePermissions;
use super::component::{ComponentRevision, PluginPriority};
use super::environment::EnvironmentId;
use super::oplog::WorkerResourceId;
use super::regions::OplogRegion;
use super::{Timestamp, WorkerId, WorkerResourceDescription, WorkerStatus};
use crate::model::OplogIndex;
use crate::{declare_enums, declare_structs, declare_unions};
use desert_rust::BinaryCodec;
use golem_wasm::{FromValue, IntoValue, Value};
use golem_wasm_derive::{FromValue, IntoValue};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
pub struct WorkerCreationRequest {
    pub name: String,
    pub env: HashMap<String, String>,
    #[oai(default)]
    pub config_vars: WasiConfigVars,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(poem_openapi::Object)]
#[oai(rename_all = "camelCase")]
pub struct WasiConfigVarsEntry {
    pub key: String,
    pub value: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, poem_openapi::NewType,
)]
#[oai(from_multipart = false, from_parameter = false, to_header = false)]
pub struct WasiConfigVars(pub Vec<WasiConfigVarsEntry>);

impl Default for WasiConfigVars {
    fn default() -> Self {
        Self::new()
    }
}

impl WasiConfigVars {
    pub fn new() -> Self {
        Self(Vec::new())
    }
}

impl From<WasiConfigVars> for BTreeMap<String, String> {
    fn from(value: WasiConfigVars) -> Self {
        value.0.into_iter().map(|e| (e.key, e.value)).collect()
    }
}

impl From<BTreeMap<String, String>> for WasiConfigVars {
    fn from(value: BTreeMap<String, String>) -> Self {
        Self(
            value
                .into_iter()
                .map(|(key, value)| WasiConfigVarsEntry { key, value })
                .collect(),
        )
    }
}

impl IntoValue for WasiConfigVars {
    fn get_type() -> golem_wasm::analysis::AnalysedType {
        BTreeMap::<String, String>::get_type()
    }
    fn into_value(self) -> golem_wasm::Value {
        BTreeMap::from(self).into_value()
    }
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

    #[derive(BinaryCodec, IntoValue, FromValue)]
    #[desert(evolution())]
    pub enum RevertWorkerTarget {
        RevertToOplogIndex(RevertToOplogIndex),
        RevertLastInvocations(RevertLastInvocations),
    }
}

declare_structs! {
    pub struct PendingUpdate {
        pub timestamp: Timestamp,
        pub target_version: ComponentRevision,
    }

    pub struct SuccessfulUpdate {
        pub timestamp: Timestamp,
        pub target_version: ComponentRevision,
    }

    pub struct FailedUpdate {
        pub timestamp: Timestamp,
        pub target_version: ComponentRevision,
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
        pub wasi_config_vars: WasiConfigVars,
        pub status: WorkerStatus,
        pub component_version: ComponentRevision,
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

    #[derive(BinaryCodec, IntoValue, FromValue)]
    #[desert(evolution())]
    pub struct RevertToOplogIndex {
        pub last_oplog_index: OplogIndex,
    }

    #[derive(BinaryCodec, IntoValue, FromValue)]
    #[desert(evolution())]
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

impl FromValue for WasiConfigVars {
    fn from_value(value: Value) -> Result<Self, String> {
        let value = BTreeMap::<String, String>::from_value(value)?;
        Ok(value.into())
    }
}

mod protobuf {
    use super::WorkerMetadataDto;
    use super::{
        ExportedResourceMetadata, FailedUpdate, PendingUpdate, SuccessfulUpdate, UpdateRecord,
        WasiConfigVars,
    };
    use super::{
        RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget, WasiConfigVarsEntry,
        WorkerUpdateMode,
    };
    use crate::model::component::{ComponentRevision, PluginPriority};
    use crate::model::oplog::WorkerResourceId;
    use crate::model::regions::OplogRegion;
    use crate::model::{OplogIndex, WorkerResourceDescription};
    use std::collections::HashSet;

    impl From<golem_api_grpc::proto::golem::worker::WasiConfigVars> for WasiConfigVars {
        fn from(value: golem_api_grpc::proto::golem::worker::WasiConfigVars) -> Self {
            Self(
                value
                    .entries
                    .into_iter()
                    .map(|e| WasiConfigVarsEntry {
                        key: e.key,
                        value: e.value,
                    })
                    .collect(),
            )
        }
    }

    impl From<WasiConfigVars> for golem_api_grpc::proto::golem::worker::WasiConfigVars {
        fn from(value: WasiConfigVars) -> Self {
            Self {
                entries: value
                    .0
                    .into_iter()
                    .map(
                        |e| golem_api_grpc::proto::golem::worker::WasiConfigVarsEntry {
                            key: e.key,
                            value: e.value,
                        },
                    )
                    .collect(),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerMetadata> for WorkerMetadataDto {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::WorkerMetadata,
        ) -> Result<Self, Self::Error> {
            let mut exported_resource_instances = Vec::new();

            for desc in value.owned_resources {
                exported_resource_instances.push(ExportedResourceMetadata {
                    key: WorkerResourceId(desc.resource_id),
                    description: WorkerResourceDescription {
                        created_at: desc.created_at.ok_or("Missing created_at")?.into(),
                        resource_owner: desc.resource_owner,
                        resource_name: desc.resource_name,
                    },
                });
            }
            Ok(Self {
                worker_id: value.worker_id.ok_or("Missing worker_id")?.try_into()?,
                environment_id: value
                    .environment_id
                    .ok_or("Missing environment_id")?
                    .try_into()?,
                created_by: value.created_by.ok_or("Missing account_id")?.try_into()?,
                env: value.env,
                wasi_config_vars: value
                    .wasi_config_vars
                    .ok_or("Missing wasi_config_vars field")?
                    .into(),
                status: value.status.try_into()?,
                component_version: ComponentRevision(value.component_version),
                retry_count: value.retry_count,
                pending_invocation_count: value.pending_invocation_count,
                updates: value
                    .updates
                    .into_iter()
                    .map(|update| update.try_into())
                    .collect::<Result<Vec<UpdateRecord>, String>>()?,
                created_at: value.created_at.ok_or("Missing created_at")?.into(),
                last_error: value.last_error,
                component_size: value.component_size,
                total_linear_memory_size: value.total_linear_memory_size,
                exported_resource_instances,
                active_plugins: value
                    .active_plugins
                    .into_iter()
                    .map(PluginPriority)
                    .collect::<HashSet<_, _>>(),
                skipped_regions: value
                    .skipped_regions
                    .into_iter()
                    .map(OplogRegion::from)
                    .collect::<Vec<_>>(),
                deleted_regions: value
                    .deleted_regions
                    .into_iter()
                    .map(OplogRegion::from)
                    .collect::<Vec<_>>(),
            })
        }
    }

    impl From<WorkerMetadataDto> for golem_api_grpc::proto::golem::worker::WorkerMetadata {
        fn from(value: WorkerMetadataDto) -> Self {
            let mut owned_resources = Vec::new();
            for instance in value.exported_resource_instances {
                owned_resources.push(golem_api_grpc::proto::golem::worker::ResourceDescription {
                    resource_id: instance.key.0,
                    resource_name: instance.description.resource_name,
                    resource_owner: instance.description.resource_owner,
                    created_at: Some(instance.description.created_at.into()),
                });
            }

            Self {
                worker_id: Some(value.worker_id.into()),
                environment_id: Some(value.environment_id.into()),
                created_by: Some(value.created_by.into()),
                env: value.env,
                wasi_config_vars: Some(value.wasi_config_vars.into()),
                status: value.status.into(),
                component_version: value.component_version.0,
                retry_count: value.retry_count,
                pending_invocation_count: value.pending_invocation_count,
                updates: value.updates.iter().cloned().map(|u| u.into()).collect(),
                created_at: Some(value.created_at.into()),
                last_error: value.last_error,
                component_size: value.component_size,
                total_linear_memory_size: value.total_linear_memory_size,
                owned_resources,
                active_plugins: value.active_plugins.into_iter().map(|id| id.0).collect(),
                skipped_regions: value
                    .skipped_regions
                    .into_iter()
                    .map(|region| region.into())
                    .collect(),
                deleted_regions: value
                    .deleted_regions
                    .into_iter()
                    .map(|region| region.into())
                    .collect(),
            }
        }
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
                        target_version: ComponentRevision(value.target_version),
                        details: { failed.details },
                    }))
                }
                golem_api_grpc::proto::golem::worker::update_record::Update::Pending(_) => {
                    Ok(Self::PendingUpdate(PendingUpdate {
                        timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                        target_version: ComponentRevision(value.target_version),
                    }))
                }
                golem_api_grpc::proto::golem::worker::update_record::Update::Successful(_) => {
                    Ok(Self::SuccessfulUpdate(SuccessfulUpdate {
                        timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                        target_version: ComponentRevision(value.target_version),
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
                    target_version: target_version.0,
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
                    target_version: target_version.0,
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
                    target_version: target_version.0,
                    update: Some(
                        golem_api_grpc::proto::golem::worker::update_record::Update::Successful(
                            golem_api_grpc::proto::golem::worker::SuccessfulUpdate {},
                        ),
                    ),
                },
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::common::RevertToOplogIndex> for RevertToOplogIndex {
        fn from(value: golem_api_grpc::proto::golem::common::RevertToOplogIndex) -> Self {
            Self {
                last_oplog_index: OplogIndex::from_u64(value.last_oplog_index as u64),
            }
        }
    }

    impl From<RevertToOplogIndex> for golem_api_grpc::proto::golem::common::RevertToOplogIndex {
        fn from(value: RevertToOplogIndex) -> Self {
            Self {
                last_oplog_index: u64::from(value.last_oplog_index) as i64,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::common::RevertLastInvocations> for RevertLastInvocations {
        fn from(value: golem_api_grpc::proto::golem::common::RevertLastInvocations) -> Self {
            Self {
                number_of_invocations: value.number_of_invocations as u64,
            }
        }
    }

    impl From<RevertLastInvocations> for golem_api_grpc::proto::golem::common::RevertLastInvocations {
        fn from(value: RevertLastInvocations) -> Self {
            Self {
                number_of_invocations: value.number_of_invocations as i64,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::common::RevertWorkerTarget> for RevertWorkerTarget {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::common::RevertWorkerTarget,
        ) -> Result<Self, Self::Error> {
            match value.target {
                Some(golem_api_grpc::proto::golem::common::revert_worker_target::Target::RevertToOplogIndex(target)) => {
                    Ok(RevertWorkerTarget::RevertToOplogIndex(target.into()))
                }
                Some(golem_api_grpc::proto::golem::common::revert_worker_target::Target::RevertLastInvocations(target)) => {
                    Ok(RevertWorkerTarget::RevertLastInvocations(target.into()))
                }
                None => Err("Missing field: target".to_string()),
            }
        }
    }

    impl From<RevertWorkerTarget> for golem_api_grpc::proto::golem::common::RevertWorkerTarget {
        fn from(value: RevertWorkerTarget) -> Self {
            match value {
                RevertWorkerTarget::RevertToOplogIndex(target) => Self {
                    target: Some(golem_api_grpc::proto::golem::common::revert_worker_target::Target::RevertToOplogIndex(target.into())),
                },
                RevertWorkerTarget::RevertLastInvocations(target) => Self {
                    target: Some(golem_api_grpc::proto::golem::common::revert_worker_target::Target::RevertLastInvocations(target.into())),
                },
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::UpdateMode> for WorkerUpdateMode {
        fn from(value: golem_api_grpc::proto::golem::worker::UpdateMode) -> Self {
            match value {
                golem_api_grpc::proto::golem::worker::UpdateMode::Automatic => {
                    WorkerUpdateMode::Automatic
                }
                golem_api_grpc::proto::golem::worker::UpdateMode::Manual => {
                    WorkerUpdateMode::Manual
                }
            }
        }
    }

    impl From<WorkerUpdateMode> for golem_api_grpc::proto::golem::worker::UpdateMode {
        fn from(value: WorkerUpdateMode) -> Self {
            match value {
                WorkerUpdateMode::Automatic => {
                    golem_api_grpc::proto::golem::worker::UpdateMode::Automatic
                }
                WorkerUpdateMode::Manual => {
                    golem_api_grpc::proto::golem::worker::UpdateMode::Manual
                }
            }
        }
    }
}
