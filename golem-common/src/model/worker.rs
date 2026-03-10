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

use super::agent::AgentTypeName;
use super::component_metadata::ComponentMetadata;
pub use crate::base_model::worker::*;
use crate::model::agent::{ConfigKeyValueType, ConfigValueType};
use golem_wasm::ValueAndType;

impl UntypedParsedWorkerCreationLocalAgentConfigEntry {
    pub fn enrich_with_type(
        self,
        component_metadata: &ComponentMetadata,
        agent_type_name: Option<&AgentTypeName>,
    ) -> Result<ParsedWorkerCreationLocalAgentConfigEntry, String> {
        let agent_type_name = agent_type_name.ok_or_else(|| {
            "cannot enrich local agent config for non-agentic workers".to_string()
        })?;

        let value_type = component_metadata
            .find_agent_type_by_name(agent_type_name)
            .ok_or("did not find expected agent type in the metadata")?
            .config
            .into_iter()
            .find_map(|c| match c {
                ConfigKeyValueType {
                    key,
                    value: ConfigValueType::Local(inner),
                } if key == self.key => Some(inner),
                _ => None,
            })
            .ok_or_else(|| {
                format!(
                    "did not find config key {} in the metadata",
                    self.key.join(".")
                )
            })?;

        Ok(ParsedWorkerCreationLocalAgentConfigEntry {
            key: self.key,
            value: ValueAndType::new(self.value, value_type.value),
        })
    }
}

mod protobuf {
    use super::AgentMetadataDto;
    use super::{AgentUpdateMode, RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget};
    use super::{
        ExportedResourceMetadata, FailedUpdate, ParsedWorkerCreationLocalAgentConfigEntry,
        PendingUpdate, SuccessfulUpdate, UpdateRecord, WorkerCreationLocalAgentConfigEntry,
    };
    use crate::model::component::PluginPriority;
    use crate::model::oplog::AgentResourceId;
    use crate::model::regions::OplogRegion;
    use crate::model::{AgentResourceDescription, OplogIndex};
    use std::collections::HashSet;

    impl TryFrom<golem_api_grpc::proto::golem::worker::AgentMetadata> for AgentMetadataDto {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::AgentMetadata,
        ) -> Result<Self, Self::Error> {
            let mut exported_resource_instances = Vec::new();

            for desc in value.owned_resources {
                exported_resource_instances.push(ExportedResourceMetadata {
                    key: AgentResourceId(desc.resource_id),
                    description: AgentResourceDescription {
                        created_at: desc.created_at.ok_or("Missing created_at")?.into(),
                        resource_owner: desc.resource_owner,
                        resource_name: desc.resource_name,
                    },
                });
            }
            Ok(Self {
                agent_id: value.agent_id.ok_or("Missing agent_id")?.try_into()?,
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

    impl From<AgentMetadataDto> for golem_api_grpc::proto::golem::worker::AgentMetadata {
        fn from(value: AgentMetadataDto) -> Self {
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
                agent_id: Some(value.agent_id.into()),
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

    impl From<golem_api_grpc::proto::golem::worker::UpdateMode> for AgentUpdateMode {
        fn from(value: golem_api_grpc::proto::golem::worker::UpdateMode) -> Self {
            match value {
                golem_api_grpc::proto::golem::worker::UpdateMode::Automatic => {
                    AgentUpdateMode::Automatic
                }
                golem_api_grpc::proto::golem::worker::UpdateMode::Manual => AgentUpdateMode::Manual,
            }
        }
    }

    impl From<AgentUpdateMode> for golem_api_grpc::proto::golem::worker::UpdateMode {
        fn from(value: AgentUpdateMode) -> Self {
            match value {
                AgentUpdateMode::Automatic => {
                    golem_api_grpc::proto::golem::worker::UpdateMode::Automatic
                }
                AgentUpdateMode::Manual => golem_api_grpc::proto::golem::worker::UpdateMode::Manual,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::LocalAgentConfigEntry>
        for WorkerCreationLocalAgentConfigEntry
    {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::worker::LocalAgentConfigEntry,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                key: value.key,
                value: serde_json::from_str::<serde_json::Value>(&value.value)
                    .map_err(|e| e.to_string())?,
            })
        }
    }

    impl From<WorkerCreationLocalAgentConfigEntry>
        for golem_api_grpc::proto::golem::worker::LocalAgentConfigEntry
    {
        fn from(value: WorkerCreationLocalAgentConfigEntry) -> Self {
            Self {
                key: value.key,
                value: serde_json::to_string(&value.value)
                    .expect("json value should be encodable to string"),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::ParsedLocalAgentConfigEntry>
        for ParsedWorkerCreationLocalAgentConfigEntry
    {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::worker::ParsedLocalAgentConfigEntry,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                key: value.key,
                value: value
                    .value
                    .ok_or_else(|| "Missing field: value".to_string())?
                    .try_into()?,
            })
        }
    }

    impl From<ParsedWorkerCreationLocalAgentConfigEntry>
        for golem_api_grpc::proto::golem::worker::ParsedLocalAgentConfigEntry
    {
        fn from(value: ParsedWorkerCreationLocalAgentConfigEntry) -> Self {
            Self {
                key: value.key,
                value: Some(value.value.into()),
            }
        }
    }
}
