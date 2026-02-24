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
use crate::services::auth::AuthService;
use crate::services::mcp_deployment::McpDeploymentService;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::mcp_deployment::{McpDeployment, McpDeploymentCreation};
use golem_common::model::poem::NoContentResponse;
use golem_common::model::Page;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::{AuthCtx, GolemSecurityScheme};
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct McpDeploymentsApi {
    mcp_deployment_service: Arc<McpDeploymentService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::McpDeployment
)]
#[allow(unused_variables)]
impl McpDeploymentsApi {
    pub fn new(
        mcp_deployment_service: Arc<McpDeploymentService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            mcp_deployment_service,
            auth_service,
        }
    }

    /// Create a new MCP deployment in the environment
    #[oai(
        path = "/envs/:environment_id/mcp-deployments",
        method = "post",
        operation_id = "create_mcp_deployment",
        tag = ApiTags::Environment,
    )]
    async fn create_mcp_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<McpDeploymentCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpDeployment>> {
        let record = recorded_http_api_request!(
            "create_mcp_deployment",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_mcp_deployment_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_mcp_deployment_internal(
        &self,
        environment_id: EnvironmentId,
        payload: McpDeploymentCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<McpDeployment>> {
        let result = self
            .mcp_deployment_service
            .create(environment_id, payload, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Get MCP deployment by domain in the environment
    #[oai(
        path = "/envs/:environment_id/mcp-deployments/:domain",
        method = "get",
        operation_id = "get_mcp_deployment_in_environment",
        tag = ApiTags::Environment
    )]
    async fn get_mcp_deployment_in_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        domain: Path<Domain>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpDeployment>> {
        let record = recorded_http_api_request!(
            "get_mcp_deployment_in_environment",
            environment_id = environment_id.0.to_string(),
            domain = domain.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_mcp_deployment_in_environment_internal(environment_id.0, domain.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_mcp_deployment_in_environment_internal(
        &self,
        environment_id: EnvironmentId,
        domain: Domain,
        auth: AuthCtx,
    ) -> ApiResult<Json<McpDeployment>> {
        let mcp_deployment = self
            .mcp_deployment_service
            .get_staged_by_domain(environment_id, &domain, &auth)
            .await?;

        Ok(Json(mcp_deployment))
    }

    /// List MCP deployments in the environment
    #[oai(
        path = "/envs/:environment_id/mcp-deployments",
        method = "get",
        operation_id = "list_mcp_deployments_in_environment",
        tag = ApiTags::Environment
    )]
    async fn list_mcp_deployments_in_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<McpDeployment>>> {
        let record = recorded_http_api_request!(
            "list_mcp_deployments_in_environment",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_mcp_deployments_in_environment_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_mcp_deployments_in_environment_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<McpDeployment>>> {
        let values = self
            .mcp_deployment_service
            .list_staged(environment_id, &auth)
            .await?;

        Ok(Json(Page { values }))
    }

    /// Get MCP deployment by ID
    #[oai(
        path = "/mcp-deployments/:mcp_deployment_id",
        method = "get",
        operation_id = "get_mcp_deployment"
    )]
    async fn get_mcp_deployment(
        &self,
        mcp_deployment_id: Path<golem_common::model::mcp_deployment::McpDeploymentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpDeployment>> {
        let record = recorded_http_api_request!(
            "get_mcp_deployment",
            mcp_deployment_id = mcp_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_mcp_deployment_internal(mcp_deployment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_mcp_deployment_internal(
        &self,
        mcp_deployment_id: golem_common::model::mcp_deployment::McpDeploymentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<McpDeployment>> {
        let mcp_deployment = self
            .mcp_deployment_service
            .get_staged(mcp_deployment_id, &auth)
            .await?;

        Ok(Json(mcp_deployment))
    }

    /// Update MCP deployment
    #[oai(
        path = "/mcp-deployments/:mcp_deployment_id",
        method = "patch",
        operation_id = "update_mcp_deployment"
    )]
    async fn update_mcp_deployment(
        &self,
        mcp_deployment_id: Path<golem_common::model::mcp_deployment::McpDeploymentId>,
        payload: Json<golem_common::model::mcp_deployment::McpDeploymentUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpDeployment>> {
        let record = recorded_http_api_request!(
            "update_mcp_deployment",
            mcp_deployment_id = mcp_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_mcp_deployment_internal(mcp_deployment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_mcp_deployment_internal(
        &self,
        mcp_deployment_id: golem_common::model::mcp_deployment::McpDeploymentId,
        payload: golem_common::model::mcp_deployment::McpDeploymentUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<McpDeployment>> {
        let mcp_deployment = self
            .mcp_deployment_service
            .update(mcp_deployment_id, payload, &auth)
            .await?;

        Ok(Json(mcp_deployment))
    }

    /// Delete MCP deployment
    #[oai(
        path = "/mcp-deployments/:mcp_deployment_id",
        method = "delete",
        operation_id = "delete_mcp_deployment"
    )]
    async fn delete_mcp_deployment(
        &self,
        mcp_deployment_id: Path<golem_common::model::mcp_deployment::McpDeploymentId>,
        current_revision: Query<golem_common::model::mcp_deployment::McpDeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        let record = recorded_http_api_request!(
            "delete_mcp_deployment",
            mcp_deployment_id = mcp_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_mcp_deployment_internal(mcp_deployment_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_mcp_deployment_internal(
        &self,
        mcp_deployment_id: golem_common::model::mcp_deployment::McpDeploymentId,
        current_revision: golem_common::model::mcp_deployment::McpDeploymentRevision,
        auth: AuthCtx,
    ) -> ApiResult<NoContentResponse> {
        self.mcp_deployment_service
            .delete(mcp_deployment_id, current_revision, &auth)
            .await?;

        Ok(NoContentResponse::NoContent)
    }

    /// Get a specific MCP deployment revision
    #[oai(
        path = "/mcp-deployment/:mcp_deployment_id/revisions/:revision",
        method = "get",
        operation_id = "get_mcp_deployment_revision"
    )]
    async fn get_mcp_deployment_revision(
        &self,
        mcp_deployment_id: Path<golem_common::model::mcp_deployment::McpDeploymentId>,
        revision: Path<golem_common::model::mcp_deployment::McpDeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpDeployment>> {
        let record = recorded_http_api_request!(
            "get_mcp_deployment_revision",
            mcp_deployment_id = mcp_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_mcp_deployment_revision_internal(mcp_deployment_id.0, revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_mcp_deployment_revision_internal(
        &self,
        mcp_deployment_id: golem_common::model::mcp_deployment::McpDeploymentId,
        revision: golem_common::model::mcp_deployment::McpDeploymentRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<McpDeployment>> {
        let result = self
            .mcp_deployment_service
            .get_revision(mcp_deployment_id, revision, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Get MCP deployment by domain in the deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/mcp-deployments/:domain",
        method = "get",
        operation_id = "get_mcp_deployment_in_deployment",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment,
    )]
    async fn get_mcp_deployment_in_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        domain: Path<Domain>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpDeployment>> {
        let record = recorded_http_api_request!(
            "get_mcp_deployment_in_deployment",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
            domain = domain.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_mcp_deployment_in_deployment_internal(
                environment_id.0,
                deployment_revision.0,
                domain.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_mcp_deployment_in_deployment_internal(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        domain: Domain,
        auth: AuthCtx,
    ) -> ApiResult<Json<McpDeployment>> {
        let mcp_deployment = self
            .mcp_deployment_service
            .get_in_deployment_by_domain(environment_id, deployment_revision, &domain, &auth)
            .await?;

        Ok(Json(mcp_deployment))
    }

    /// List MCP deployments by domain in the deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/mcp-deployments",
        method = "get",
        operation_id = "list_mcp_deployments_in_deployment",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment,
    )]
    async fn list_mcp_deployments_in_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<McpDeployment>>> {
        let record = recorded_http_api_request!(
            "list_mcp_deployments_in_deployment",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_mcp_deployments_in_deployment_internal(
                environment_id.0,
                deployment_revision.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_mcp_deployments_in_deployment_internal(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<McpDeployment>>> {
        let values = self
            .mcp_deployment_service
            .list_in_deployment(environment_id, deployment_revision, &auth)
            .await?;

        Ok(Json(Page { values }))
    }

}
