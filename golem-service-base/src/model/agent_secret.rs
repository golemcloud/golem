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

use golem_common::model::agent_secret::{AgentSecretId, AgentSecretRevision};
use golem_common::model::environment::EnvironmentId;
use golem_wasm::ValueAndType;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;

#[derive(Debug, Clone)]
pub struct AgentSecret {
    pub id: AgentSecretId,
    pub environment_id: EnvironmentId,
    pub path: Vec<String>,
    pub revision: AgentSecretRevision,
    pub secret_type: AnalysedType,
    pub secret_value: Option<golem_wasm::Value>,
}

impl From<AgentSecret> for golem_common::model::agent_secret::AgentSecretDto {
    fn from(value: AgentSecret) -> Self {
        Self {
            id: value.id,
            environment_id: value.environment_id,
            path: value.path,
            revision: value.revision,
            secret_value: value.secret_value.map(|sv| {
                let value_and_type = ValueAndType::new(sv, value.secret_type.clone());
                value_and_type
                    .to_json_value()
                    .expect("value and type in AgentSecret must be valid JSON")
            }),
            secret_type: value.secret_type,
        }
    }
}

impl From<AgentSecret> for golem_api_grpc::proto::golem::registry::AgentSecret {
    fn from(value: AgentSecret) -> Self {
        Self {
            agent_secret_id: Some(value.id.into()),
            environment_id: Some(value.environment_id.into()),
            path: value.path,
            revision: value.revision.into(),
            secret_type: Some((&value.secret_type).into()),
            secret_value: value.secret_value.map(Into::into),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::AgentSecret> for AgentSecret {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::registry::AgentSecret,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value
                .agent_secret_id
                .ok_or("Missing agent_secret_id field")?
                .try_into()?,
            environment_id: value
                .environment_id
                .ok_or("Missing environment_id field")?
                .try_into()?,
            path: value.path,
            revision: AgentSecretRevision::try_from(value.revision)?,
            secret_type: (&value.secret_type.ok_or("Missing secret_type field")?).try_into()?,
            secret_value: value.secret_value.map(TryInto::try_into).transpose()?,
        })
    }
}
