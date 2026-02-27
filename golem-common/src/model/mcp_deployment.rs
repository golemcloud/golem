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

pub use crate::base_model::mcp_deployment::*;
use crate::model::diff;

impl McpDeployment {
    pub fn to_diffable(&self) -> diff::McpDeployment {
        diff::McpDeployment {
            agents: self
                .agents
                .iter()
                .map(|(k, _v)| (k.0.clone(), diff::McpDeploymentAgentOptions::default()))
                .collect(),
        }
    }
}
