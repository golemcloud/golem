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

use golem_common::model::agent::AgentType;
use golem_common::model::ComponentId;
use poem_openapi_derive::Object;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct RegisteredAgentType {
    pub agent_type: AgentType,
    pub implemented_by: ComponentId,
}

impl TryFrom<golem_api_grpc::proto::golem::component::v1::RegisteredAgentType>
    for RegisteredAgentType
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::v1::RegisteredAgentType,
    ) -> Result<Self, Self::Error> {
        Ok(RegisteredAgentType {
            agent_type: value
                .agent_type
                .ok_or_else(|| "Missing agent_type field".to_string())?
                .try_into()?,
            implemented_by: value
                .implemented_by
                .ok_or_else(|| "Missing implemented_by field".to_string())?
                .try_into()?,
        })
    }
}

impl From<RegisteredAgentType>
    for golem_api_grpc::proto::golem::component::v1::RegisteredAgentType
{
    fn from(value: RegisteredAgentType) -> Self {
        golem_api_grpc::proto::golem::component::v1::RegisteredAgentType {
            agent_type: Some(value.agent_type.into()),
            implemented_by: Some(value.implemented_by.into()),
        }
    }
}
