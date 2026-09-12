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

use crate::model::diff;

pub use crate::base_model::deploy_validation_warning::*;
pub use crate::base_model::deployment::*;

impl From<CurrentDeployment> for Deployment {
    fn from(value: CurrentDeployment) -> Self {
        Self {
            environment_id: value.environment_id,
            revision: value.revision,
            version: value.version,
            deployment_hash: value.deployment_hash,
        }
    }
}

impl DeploymentPlan {
    pub fn to_diffable(&self) -> diff::Deployment {
        diff::Deployment {
            components: self
                .components
                .iter()
                .map(|component| (component.name.0.clone(), component.hash.into()))
                .collect(),
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|had| (had.domain.0.clone(), had.hash.into()))
                .collect(),
            mcp_deployments: self
                .mcp_deployments
                .iter()
                .map(|mcd| (mcd.domain.0.clone(), mcd.hash.into()))
                .collect(),
            remote_tools: self
                .remote_tools
                .iter()
                .map(|tool| (tool.name.to_string(), tool.hash.into()))
                .collect(),
            published_tools: self
                .published_tools
                .iter()
                .map(ToString::to_string)
                .collect(),
            remote_tool_middleware_deployments: self
                .remote_tool_middlewares
                .iter()
                .map(|e| (e.name.to_string(), diff::HashOf::from_hash(e.hash)))
                .collect(),
            published_tool_middlewares: self
                .published_tool_middlewares
                .iter()
                .map(ToString::to_string)
                .collect(),
            universal_tool_middlewares: self.universal_tool_middlewares.clone(),
            tool_compatibility_mode: self.tool_compatibility_mode,
            environment_tool_middleware_bindings: self
                .environment_tool_middleware_bindings
                .iter()
                .map(|(n, b)| (n.to_string(), b.into()))
                .collect(),
            agent_tool_middleware_bindings: self
                .agent_tool_middleware_bindings
                .iter()
                .map(|(a, bs)| {
                    (
                        a.0.clone(),
                        bs.iter().map(|(n, b)| (n.to_string(), b.into())).collect(),
                    )
                })
                .collect(),
        }
    }
}

impl DeploymentSummary {
    pub fn to_diffable(&self) -> diff::Deployment {
        diff::Deployment {
            components: self
                .components
                .iter()
                .map(|component| (component.name.0.clone(), component.hash.into()))
                .collect(),
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|had| (had.domain.0.clone(), had.hash.into()))
                .collect(),
            mcp_deployments: self
                .mcp_deployments
                .iter()
                .map(|mcd| (mcd.domain.0.clone(), mcd.hash.into()))
                .collect(),
            remote_tools: self
                .remote_tools
                .iter()
                .map(|tool| (tool.name.to_string(), tool.hash.into()))
                .collect(),
            published_tools: self
                .published_tools
                .iter()
                .map(ToString::to_string)
                .collect(),
            remote_tool_middleware_deployments: self
                .remote_tool_middlewares
                .iter()
                .map(|e| (e.name.to_string(), diff::HashOf::from_hash(e.hash)))
                .collect(),
            published_tool_middlewares: self
                .published_tool_middlewares
                .iter()
                .map(ToString::to_string)
                .collect(),
            universal_tool_middlewares: self.universal_tool_middlewares.clone(),
            tool_compatibility_mode: self.tool_compatibility_mode,
            environment_tool_middleware_bindings: self
                .environment_tool_middleware_bindings
                .iter()
                .map(|(n, b)| (n.to_string(), b.into()))
                .collect(),
            agent_tool_middleware_bindings: self
                .agent_tool_middleware_bindings
                .iter()
                .map(|(a, bs)| {
                    (
                        a.0.clone(),
                        bs.iter().map(|(n, b)| (n.to_string(), b.into())).collect(),
                    )
                })
                .collect(),
        }
    }
}
