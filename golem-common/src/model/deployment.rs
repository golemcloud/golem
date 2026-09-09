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

use crate::model::agent::AgentTypeName;
use crate::model::diff;
use crate::model::tool::ToolBindingInput;
use std::collections::{BTreeMap, BTreeSet};

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
        let remote_tools: std::collections::BTreeMap<_, _> = self
            .remote_tools
            .iter()
            .map(|tool| (tool.name.to_string(), tool.hash.into()))
            .collect();
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
            remote_tools,
            published_tools: self
                .published_tools
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }
}

impl DeploymentPlanAmbientToolEntry {
    pub fn to_diffable(
        &self,
        agent_types: impl IntoIterator<Item = AgentTypeName>,
        overrides: &BTreeMap<AgentTypeName, ToolBindingInput>,
    ) -> diff::RemoteToolDeployment {
        let bindings = agent_types
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|agent| {
                diff::effective_tool_binding(Some(&self.environment_binding), overrides.get(&agent))
                    .map(|(binding, _)| (agent, binding))
            })
            .collect();
        diff::RemoteToolDeployment {
            release_id: self.release_id,
            version: self.version.0.clone(),
            source_digest: self.source_digest,
            owner_account_id: self.owner_account_id,
            owner_account_email: self.owner_account_email.clone(),
            metadata_version: self.metadata_version.0.clone(),
            metadata_digest: self.metadata_digest,
            provision: self.provision.clone(),
            bindings,
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
        }
    }
}
