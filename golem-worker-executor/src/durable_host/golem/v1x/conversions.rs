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

use crate::preview2::golem_api_1_x;
use crate::preview2::golem_api_1_x::host::AgentAnyFilter;
use golem_common::model::OplogIndex;
use golem_common::model::oplog::types::AgentMetadataForGuests;

impl From<golem_api_1_x::host::RevertAgentTarget>
    for golem_common::model::agent::RevertWorkerTarget
{
    fn from(value: golem_api_1_x::host::RevertAgentTarget) -> Self {
        match value {
            golem_api_1_x::host::RevertAgentTarget::RevertToOplogIndex(index) => {
                golem_common::model::agent::RevertWorkerTarget::RevertToOplogIndex(
                    golem_common::model::agent::RevertToOplogIndex {
                        last_oplog_index: OplogIndex::from_u64(index),
                    },
                )
            }
            golem_api_1_x::host::RevertAgentTarget::RevertLastInvocations(n) => {
                golem_common::model::agent::RevertWorkerTarget::RevertLastInvocations(
                    golem_common::model::agent::RevertLastInvocations {
                        number_of_invocations: n,
                    },
                )
            }
        }
    }
}

impl From<golem_api_1_x::host::FilterComparator> for golem_common::model::FilterComparator {
    fn from(value: golem_api_1_x::host::FilterComparator) -> Self {
        match value {
            golem_api_1_x::host::FilterComparator::Equal => {
                golem_common::model::FilterComparator::Equal
            }
            golem_api_1_x::host::FilterComparator::NotEqual => {
                golem_common::model::FilterComparator::NotEqual
            }
            golem_api_1_x::host::FilterComparator::Less => {
                golem_common::model::FilterComparator::Less
            }
            golem_api_1_x::host::FilterComparator::LessEqual => {
                golem_common::model::FilterComparator::LessEqual
            }
            golem_api_1_x::host::FilterComparator::Greater => {
                golem_common::model::FilterComparator::Greater
            }
            golem_api_1_x::host::FilterComparator::GreaterEqual => {
                golem_common::model::FilterComparator::GreaterEqual
            }
        }
    }
}

impl From<golem_api_1_x::host::StringFilterComparator>
    for golem_common::model::StringFilterComparator
{
    fn from(value: golem_api_1_x::host::StringFilterComparator) -> Self {
        match value {
            golem_api_1_x::host::StringFilterComparator::Equal => {
                golem_common::model::StringFilterComparator::Equal
            }
            golem_api_1_x::host::StringFilterComparator::NotEqual => {
                golem_common::model::StringFilterComparator::NotEqual
            }
            golem_api_1_x::host::StringFilterComparator::Like => {
                golem_common::model::StringFilterComparator::Like
            }
            golem_api_1_x::host::StringFilterComparator::NotLike => {
                golem_common::model::StringFilterComparator::NotLike
            }
            golem_api_1_x::host::StringFilterComparator::StartsWith => {
                golem_common::model::StringFilterComparator::StartsWith
            }
        }
    }
}

impl From<golem_api_1_x::host::AgentStatus> for golem_common::model::AgentStatus {
    fn from(value: golem_api_1_x::host::AgentStatus) -> Self {
        match value {
            golem_api_1_x::host::AgentStatus::Running => golem_common::model::AgentStatus::Running,
            golem_api_1_x::host::AgentStatus::Idle => golem_common::model::AgentStatus::Idle,
            golem_api_1_x::host::AgentStatus::Suspended => {
                golem_common::model::AgentStatus::Suspended
            }
            golem_api_1_x::host::AgentStatus::Interrupted => {
                golem_common::model::AgentStatus::Interrupted
            }
            golem_api_1_x::host::AgentStatus::Retrying => {
                golem_common::model::AgentStatus::Retrying
            }
            golem_api_1_x::host::AgentStatus::Failed => golem_common::model::AgentStatus::Failed,
            golem_api_1_x::host::AgentStatus::Exited => golem_common::model::AgentStatus::Exited,
        }
    }
}

impl From<golem_common::model::AgentStatus> for golem_api_1_x::host::AgentStatus {
    fn from(value: golem_common::model::AgentStatus) -> Self {
        match value {
            golem_common::model::AgentStatus::Running => golem_api_1_x::host::AgentStatus::Running,
            golem_common::model::AgentStatus::Idle => golem_api_1_x::host::AgentStatus::Idle,
            golem_common::model::AgentStatus::Suspended => {
                golem_api_1_x::host::AgentStatus::Suspended
            }
            golem_common::model::AgentStatus::Interrupted => {
                golem_api_1_x::host::AgentStatus::Interrupted
            }
            golem_common::model::AgentStatus::Retrying => {
                golem_api_1_x::host::AgentStatus::Retrying
            }
            golem_common::model::AgentStatus::Failed => golem_api_1_x::host::AgentStatus::Failed,
            golem_common::model::AgentStatus::Exited => golem_api_1_x::host::AgentStatus::Exited,
        }
    }
}

impl TryFrom<golem_api_1_x::host::AgentPropertyFilter> for golem_common::model::AgentFilter {
    type Error = String;

    fn try_from(filter: golem_api_1_x::host::AgentPropertyFilter) -> Result<Self, Self::Error> {
        let converted = match filter {
            golem_api_1_x::host::AgentPropertyFilter::Name(filter) => {
                golem_common::model::AgentFilter::new_name(filter.comparator.into(), filter.value)
            }
            golem_api_1_x::host::AgentPropertyFilter::Version(filter) => {
                golem_common::model::AgentFilter::new_revision(
                    filter.comparator.into(),
                    filter.value.try_into()?,
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::Status(filter) => {
                golem_common::model::AgentFilter::new_status(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::Env(filter) => {
                golem_common::model::AgentFilter::new_env(
                    filter.name,
                    filter.comparator.into(),
                    filter.value,
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::CreatedAt(filter) => {
                golem_common::model::AgentFilter::new_created_at(
                    filter.comparator.into(),
                    filter.value.into(),
                )
            }
            golem_api_1_x::host::AgentPropertyFilter::Config(filter) => {
                golem_common::model::AgentFilter::new_config(
                    filter.name,
                    filter.comparator.into(),
                    filter.value,
                )
            }
        };
        Ok(converted)
    }
}

impl TryFrom<golem_api_1_x::host::AgentAllFilter> for golem_common::model::AgentFilter {
    type Error = String;
    fn try_from(filter: golem_api_1_x::host::AgentAllFilter) -> Result<Self, Self::Error> {
        let filters = filter
            .filters
            .into_iter()
            .map(|f| f.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(golem_common::model::AgentFilter::new_and(filters))
    }
}

impl TryFrom<AgentAnyFilter> for golem_common::model::AgentFilter {
    type Error = String;
    fn try_from(filter: AgentAnyFilter) -> Result<Self, Self::Error> {
        let filters = filter
            .filters
            .into_iter()
            .map(|f| f.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(golem_common::model::AgentFilter::new_or(filters))
    }
}

impl From<AgentMetadataForGuests> for golem_api_1_x::host::AgentMetadata {
    fn from(value: AgentMetadataForGuests) -> Self {
        Self {
            agent_id: value.agent_id.into(),
            args: vec![],
            env: value.env,
            config: value.config.into_iter().collect(),
            status: value.status.into(),
            component_revision: value.component_revision.into(),
            retry_count: 0,
            environment_id: value.environment_id.into(),
        }
    }
}
