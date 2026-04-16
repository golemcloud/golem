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

use super::ApiResult;
use crate::services::agent_secret::AgentSecretService;
use crate::services::auth::AuthService;
use golem_common::model::Page;
use golem_common::model::agent_secret::{
    AgentSecretCreation, AgentSecretDto, AgentSecretId, AgentSecretRevision, AgentSecretUpdate,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct AgentSecretsApi {
    agent_secret_service: Arc<AgentSecretService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::AgentSecrets
)]
impl AgentSecretsApi {
    pub fn new(
        agent_secret_service: Arc<AgentSecretService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            agent_secret_service,
            auth_service,
        }
    }

    /// Create a new agent secret
    #[oai(
        path = "/envs/:environment_id/agent-secrets",
        method = "post",
        operation_id = "create_agent_secret",
        tag = ApiTags::Environment
    )]
    async fn create_agent_secret(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<AgentSecretCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<AgentSecretDto>> {
        let record = recorded_http_api_request!(
            "create_agent_secret",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_agent_secret_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_agent_secret_internal(
        &self,
        environment_id: EnvironmentId,
        payload: AgentSecretCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<AgentSecretDto>> {
        let result = self
            .agent_secret_service
            .create(environment_id, payload, &auth)
            .await?;

        Ok(Json(result.into()))
    }

    /// List all agent secrets of the environment
    #[oai(
        path = "/envs/:environment_id/agent-secrets",
        method = "get",
        operation_id = "list_environment_agent_secrets",
        tag = ApiTags::Environment
    )]
    async fn list_environment_agent_secrets(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<AgentSecretDto>>> {
        let record = recorded_http_api_request!(
            "list_environment_agent_secrets",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_environment_agent_secrets_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_environment_agent_secrets_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<AgentSecretDto>>> {
        let result = self
            .agent_secret_service
            .list_in_environment(environment_id, &auth)
            .await?;

        let converted = result.into_iter().map(AgentSecretDto::from).collect();

        Ok(Json(Page { values: converted }))
    }

    /// Get agent secret by id.
    #[oai(
        path = "/agent-secrets/:agent_secret_id",
        method = "get",
        operation_id = "get_agent_secret"
    )]
    pub async fn get_agent_secret(
        &self,
        agent_secret_id: Path<AgentSecretId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<AgentSecretDto>> {
        let record = recorded_http_api_request!(
            "get_agent_secret",
            agent_secret_id = agent_secret_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_agent_secret_internal(agent_secret_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_agent_secret_internal(
        &self,
        agent_secret_id: AgentSecretId,
        auth: AuthCtx,
    ) -> ApiResult<Json<AgentSecretDto>> {
        let result = self
            .agent_secret_service
            .get(agent_secret_id, &auth)
            .await?;
        Ok(Json(result.into()))
    }

    /// Update agent secret
    #[oai(
        path = "/agent-secrets/:agent_secret_id",
        method = "patch",
        operation_id = "update_agent_secret"
    )]
    pub async fn update_agent_secret(
        &self,
        agent_secret_id: Path<AgentSecretId>,
        data: Json<AgentSecretUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<AgentSecretDto>> {
        let record = recorded_http_api_request!(
            "update_agent_secret",
            agent_secret_id = agent_secret_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_agent_secret_internal(agent_secret_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_agent_secret_internal(
        &self,
        agent_secret_id: AgentSecretId,
        data: AgentSecretUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<AgentSecretDto>> {
        let result = self
            .agent_secret_service
            .update(agent_secret_id, data, &auth)
            .await?;
        Ok(Json(result.into()))
    }

    /// Delete agent secret
    #[oai(
        path = "/agent-secret/:agent_secret_id",
        method = "delete",
        operation_id = "delete_agent_secret"
    )]
    pub async fn delete_agent_secret(
        &self,
        agent_secret_id: Path<AgentSecretId>,
        current_revision: Query<AgentSecretRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<AgentSecretDto>> {
        let record = recorded_http_api_request!(
            "delete_agent_secret",
            agent_secret_id = agent_secret_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_agent_secret_internal(agent_secret_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_agent_secret_internal(
        &self,
        agent_secret_id: AgentSecretId,
        current_revision: AgentSecretRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<AgentSecretDto>> {
        let result = self
            .agent_secret_service
            .delete(agent_secret_id, current_revision, &auth)
            .await?;
        Ok(Json(result.into()))
    }
}
