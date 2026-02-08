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

use crate::model::component::render_agent_constructor;
use cli_table::Table;
use golem_common::model::agent::DeployedRegisteredAgentType;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, Table)]
#[serde(rename_all = "camelCase")]
pub struct AgentTypeView {
    #[table(title = "Agent Type")]
    pub agent_type: String,
    #[table(title = "Constructor")]
    pub constructor: String,
    #[table(title = "Description")]
    pub description: String,
}

impl AgentTypeView {
    pub fn new(value: &DeployedRegisteredAgentType, wrapper_naming: bool) -> Self {
        Self {
            agent_type: value.agent_type.type_name.to_string(),
            constructor: render_agent_constructor(&value.agent_type, wrapper_naming, false),
            description: value.agent_type.description.clone(),
        }
    }
}
