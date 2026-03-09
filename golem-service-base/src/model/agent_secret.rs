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
