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

use super::agent_secret::{AgentSecretError, AgentSecretService};
use super::deployment::{DeploymentError, DeploymentService};
use golem_common::model::environment::EnvironmentId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::AgentDeploymentDetails;
use golem_service_base::model::environment::EnvironmentState;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentStateError {
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for EnvironmentStateError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(EnvironmentStateError, DeploymentError, AgentSecretError);

pub struct EnvironmentStateService {
    pub deployment_service: Arc<DeploymentService>,
    pub agent_secret_service: Arc<AgentSecretService>,
}

impl EnvironmentStateService {
    pub fn new(
        deployment_service: Arc<DeploymentService>,
        agent_secret_service: Arc<AgentSecretService>,
    ) -> Self {
        Self {
            deployment_service,
            agent_secret_service,
        }
    }

    pub async fn get_environment_state(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<EnvironmentState, EnvironmentStateError> {
        let deployed_agent_types = self
            .deployment_service
            .list_deployed_agent_types(environment_id)
            .await?;

        let agent_deployment_details = deployed_agent_types
            .into_iter()
            .map(|at| {
                (
                    at.agent_type.type_name.clone(),
                    AgentDeploymentDetails::from(at),
                )
            })
            .collect();

        let agent_secrets = self
            .agent_secret_service
            .list_in_environment_unchecked(environment_id)
            .await?
            .into_iter()
            .map(|sec| (sec.path.clone(), sec))
            .collect();

        Ok(EnvironmentState {
            agent_deployment_details,
            agent_secrets,
        })
    }
}
