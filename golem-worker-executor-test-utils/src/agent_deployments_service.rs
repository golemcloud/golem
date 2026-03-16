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

use async_trait::async_trait;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::AgentDeploymentDetails;
use golem_worker_executor::services::environment_state::EnvironmentStateService;
use std::collections::HashMap;

pub struct DisabledEnvironmentStateService;

#[async_trait]
impl EnvironmentStateService for DisabledEnvironmentStateService {
    async fn get_agent_deployment(
        &self,
        _environment: EnvironmentId,
        _agent_type: &AgentTypeName,
    ) -> Result<Option<AgentDeploymentDetails>, WorkerExecutorError> {
        unimplemented!()
    }

    async fn get_agent_secrets(
        &self,
        _environment_id: EnvironmentId,
    ) -> Result<HashMap<Vec<String>, AgentSecret>, WorkerExecutorError> {
        Ok(HashMap::new())
    }
}
