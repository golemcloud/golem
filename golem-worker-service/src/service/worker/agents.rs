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

use crate::api::agents::{AgentInvocationMode, AgentInvocationRequest, AgentInvocationResult};
use crate::service::component::ComponentService;
use crate::service::worker::{WorkerResult, WorkerService, WorkerServiceError};
use golem_service_base::clients::registry::RegistryService;
use golem_service_base::model::auth::AuthCtx;
use std::sync::Arc;
use golem_common::model::agent::{AgentId, DataValue};

pub struct AgentsService {
    registry_service: Arc<dyn RegistryService>,
    component_service: Arc<dyn ComponentService>,
    worker_service: Arc<WorkerService>,
}

impl AgentsService {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        component_service: Arc<dyn ComponentService>,
        worker_service: Arc<WorkerService>,
    ) -> Self {
        Self {
            registry_service,
            component_service,
            worker_service,
        }
    }

    pub async fn invoke_agent(
        &self,
        request: AgentInvocationRequest,
        auth: AuthCtx,
    ) -> WorkerResult<AgentInvocationResult> {
        let registered_agent_type = self
            .registry_service
            .resolve_latest_agent_type_by_names(
                &request.app_name,
                &request.env_name,
                &request.agent_type_name,
            )
            .await?;

        let component_metadata = self
            .component_service
            .get_revision(
                registered_agent_type.implemented_by.component_id,
                registered_agent_type.implemented_by.component_revision,
            )
            .await?;

        let agent_type = component_metadata
            .metadata
            .find_agent_type_by_name(&request.agent_type_name)
            .map_err(|err| {
                WorkerServiceError::Internal(format!(
                    "Cannot get agent type {} from component metadata: {err}",
                    request.agent_type_name
                ))
            })?
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent type {} not found in component metadata",
                    request.agent_type_name
                ))
            })?

        let constructor_parameters: DataValue = request.parameters

        let agent_id = AgentId::new(
            request.agent_type_name,
            constructor_parameters,
            request.phantom_id
        );

        match request.mode {
            AgentInvocationMode::Await => {
                // self.worker_service.invoke_and_await_typed(
                // )
                todo!()
            }
            AgentInvocationMode::Schedule => {
                todo!()
            }
        }
    }
}
