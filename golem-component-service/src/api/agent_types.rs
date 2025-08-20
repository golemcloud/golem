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

use crate::api::ComponentError;
use crate::authed::agent_types::AuthedAgentTypesService;
use golem_common::base_model::ProjectId;
use golem_common::model::agent::RegisteredAgentType;
use golem_common::model::auth::AuthCtx;
use golem_common::model::error::ErrorBody;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi_derive::OpenApi;
use std::sync::Arc;
use tracing::Instrument;

pub struct AgentTypesApi {
    agent_types_service: Arc<AuthedAgentTypesService>,
}

#[OpenApi(prefix_path = "/v1/agent-types", tag = ApiTags::AgentTypes)]
impl AgentTypesApi {
    pub fn new(agent_types_service: Arc<AuthedAgentTypesService>) -> Self {
        Self {
            agent_types_service,
        }
    }

    #[oai(path = "/", method = "get", operation_id = "get_all_agent_types")]
    async fn get_all_agent_types(
        &self,
        #[oai(name = "project-id")] project_id: Query<Option<ProjectId>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<RegisteredAgentType>>, ComponentError> {
        let auth = AuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "get_all_agent_types",
            project_id = project_id.0.as_ref().map(|v| v.to_string()),
        );
        let result = self
            .agent_types_service
            .get_all_agent_types(project_id.0, auth)
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(super::ComponentError::from);
        record.result(result)
    }

    #[oai(path = "/:agent-type", method = "get", operation_id = "get_agent_type")]
    async fn get_agent_type(
        &self,
        #[oai(name = "agent-type")] agent_type: Path<String>,
        #[oai(name = "project-id")] project_id: Query<Option<ProjectId>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<RegisteredAgentType>, ComponentError> {
        let auth = AuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "get_all_agent_types",
            project_id = project_id.0.as_ref().map(|v| v.to_string()),
        );
        let result = self
            .get_agent_type_internal(&agent_type.0, project_id.0, auth)
            .instrument(record.span.clone())
            .await;
        record.result(result)
    }

    async fn get_agent_type_internal(
        &self,
        agent_type: &str,
        project_id: Option<ProjectId>,
        auth: AuthCtx,
    ) -> Result<Json<RegisteredAgentType>, ComponentError> {
        let result = self
            .agent_types_service
            .get_agent_type(agent_type, project_id, auth)
            .await?;

        match result {
            Some(agent_type) => Ok(Json(agent_type)),
            None => Err(ComponentError::NotFound(Json(ErrorBody {
                error: format!("Agent type '{agent_type}' not found"),
            }))),
        }
    }
}
