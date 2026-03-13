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

use super::AgentDeploymentDetails;
use super::agent_secret::AgentSecret;
use golem_common::model::agent::AgentTypeName;
use std::collections::HashMap;

// The current, mutable state of the environment.
// The different fields in this struct are not guaranteed to come from the same snapshot.
#[derive(Debug)]
pub struct EnvironmentState {
    pub agent_deployment_details: HashMap<AgentTypeName, AgentDeploymentDetails>,
    pub agent_secrets: HashMap<Vec<String>, AgentSecret>,
}

impl From<EnvironmentState> for golem_api_grpc::proto::golem::registry::EnvironmentState {
    fn from(value: EnvironmentState) -> Self {
        Self {
            agent_deployment_details: value
                .agent_deployment_details
                .into_values()
                .map(Into::into)
                .collect(),
            agent_secrets: value.agent_secrets.into_values().map(Into::into).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::EnvironmentState> for EnvironmentState {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::registry::EnvironmentState,
    ) -> Result<Self, Self::Error> {
        let mut agent_secrets = HashMap::new();
        for entry in value.agent_secrets {
            let converted = AgentSecret::try_from(entry)?;
            agent_secrets.insert(converted.path.clone(), converted);
        }

        Ok(Self {
            agent_deployment_details: value
                .agent_deployment_details
                .into_iter()
                .map(AgentDeploymentDetails::from)
                .map(|v| (v.agent_type_name.clone(), v))
                .collect(),
            agent_secrets,
        })
    }
}
