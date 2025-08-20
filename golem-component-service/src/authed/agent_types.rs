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

use crate::authed::is_authorized_by_project_or_default;
use crate::error::ComponentError;
use crate::model::agent_types::RegisteredAgentType;
use crate::service::agent_types::AgentTypesService;
use golem_common::model::auth::{AuthCtx, ProjectAction};
use golem_common::model::ProjectId;
use golem_service_base::clients::auth::AuthService;
use golem_service_base::clients::project::ProjectService;
use std::sync::Arc;

pub struct AuthedAgentTypesService {
    agent_types_service: Arc<dyn AgentTypesService>,
    auth_service: Arc<AuthService>,
    project_service: Arc<dyn ProjectService>,
}

impl AuthedAgentTypesService {
    pub fn new(
        agent_types_service: Arc<dyn AgentTypesService>,
        auth_service: Arc<AuthService>,
        project_service: Arc<dyn ProjectService>,
    ) -> Self {
        Self {
            agent_types_service,
            auth_service,
            project_service,
        }
    }

    pub async fn get_all_agent_types(
        &self,
        project_id: Option<ProjectId>,
        auth: AuthCtx,
    ) -> Result<Vec<RegisteredAgentType>, ComponentError> {
        let owner = is_authorized_by_project_or_default(
            &self.auth_service,
            &self.project_service,
            &auth,
            project_id,
            &ProjectAction::ViewComponent,
        )
        .await?;
        let agent_types = self.agent_types_service.get_all_agent_types(&owner).await?;
        Ok(agent_types)
    }

    pub async fn get_agent_type(
        &self,
        agent_type: &str,
        project_id: Option<ProjectId>,
        auth: AuthCtx,
    ) -> Result<Option<RegisteredAgentType>, ComponentError> {
        let owner = is_authorized_by_project_or_default(
            &self.auth_service,
            &self.project_service,
            &auth,
            project_id,
            &ProjectAction::ViewComponent,
        )
        .await?;
        let agent_type = self
            .agent_types_service
            .get_agent_type(agent_type, &owner)
            .await?;
        Ok(agent_type)
    }
}
