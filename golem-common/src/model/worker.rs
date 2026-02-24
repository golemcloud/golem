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

pub use crate::base_model::worker::*;

mod protobuf {
    use super::WorkerMetadataDto;
    use super::{
        ExportedResourceMetadata, FailedUpdate, PendingUpdate, SuccessfulUpdate, UpdateRecord,
    };
    use super::{RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget, WorkerUpdateMode};
    use crate::model::component::PluginPriority;
    use crate::model::oplog::WorkerResourceId;
    use crate::model::regions::OplogRegion;
    use crate::model::{OplogIndex, WorkerResourceDescription};
    use std::collections::HashSet;

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
                config_vars: value.config_vars.into_iter().collect(),
                status: value.status.try_into()?,
                component_revision: value.component_revision.try_into()?,
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
                config_vars: value.config_vars.into_iter().collect(),
                status: value.status.into(),
                component_revision: value.component_revision.into(),
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
                        target_revision: value.target_revision.try_into()?,
                        details: { failed.details },
                    }))
                }
                golem_api_grpc::proto::golem::worker::update_record::Update::Pending(_) => {
                    Ok(Self::PendingUpdate(PendingUpdate {
                        timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                        target_revision: value.target_revision.try_into()?,
                    }))
                }
                golem_api_grpc::proto::golem::worker::update_record::Update::Successful(_) => {
                    Ok(Self::SuccessfulUpdate(SuccessfulUpdate {
                        timestamp: value.timestamp.ok_or("Missing timestamp")?.into(),
                        target_revision: value.target_revision.try_into()?,
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
                    target_revision,
                    details,
                }) => Self {
                    timestamp: Some(timestamp.into()),
                    target_revision: target_revision.into(),
                    update: Some(
                        golem_api_grpc::proto::golem::worker::update_record::Update::Failed(
                            golem_api_grpc::proto::golem::worker::FailedUpdate { details },
                        ),
                    ),
                },
                UpdateRecord::PendingUpdate(PendingUpdate {
                    timestamp,
                    target_revision,
                }) => Self {
                    timestamp: Some(timestamp.into()),
                    target_revision: target_revision.into(),
                    update: Some(
                        golem_api_grpc::proto::golem::worker::update_record::Update::Pending(
                            golem_api_grpc::proto::golem::worker::PendingUpdate {},
                        ),
                    ),
                },
                UpdateRecord::SuccessfulUpdate(SuccessfulUpdate {
                    timestamp,
                    target_revision,
                }) => Self {
                    timestamp: Some(timestamp.into()),
                    target_revision: target_revision.into(),
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
