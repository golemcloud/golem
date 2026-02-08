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
use crate::services::deployment::{DeploymentService, DeploymentWriteService};
use crate::services::environment::EnvironmentService;
use golem_common::model::Page;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::agent::DeployedRegisteredAgentType;
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::deployment::{
    CurrentDeployment, Deployment, DeploymentCreation, DeploymentPlan, DeploymentRevision,
    DeploymentRollback, DeploymentSummary, DeploymentVersion,
};
use golem_common::model::environment::*;
use golem_common::model::poem::NoContentResponse;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct EnvironmentsApi {
    environment_service: Arc<EnvironmentService>,
    deployment_service: Arc<DeploymentService>,
    deployment_write_service: Arc<DeploymentWriteService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Environment
)]
impl EnvironmentsApi {
    pub fn new(
        environment_service: Arc<EnvironmentService>,
        deployment_service: Arc<DeploymentService>,
        deployment_write_service: Arc<DeploymentWriteService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            environment_service,
            deployment_service,
            deployment_write_service,
            auth_service,
        }
    }

    /// Create an application environment
    #[oai(
        path = "/apps/:application_id/envs",
        method = "post",
        operation_id = "create_environment",
        tag = ApiTags::Application
    )]
    pub async fn create_environment(
        &self,
        application_id: Path<ApplicationId>,
        data: Json<EnvironmentCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "create_environment",
            application_id = application_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_environment_internal(application_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_environment_internal(
        &self,
        application_id: ApplicationId,
        data: EnvironmentCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        let result = self
            .environment_service
            .create(application_id, data, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Get application environment by name
    #[oai(
        path = "/apps/:application_id/envs/:environment_name",
        method = "get",
        operation_id = "get_application_environment",
        tag = ApiTags::Application
    )]
    pub async fn get_application_environment(
        &self,
        application_id: Path<ApplicationId>,
        environment_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "get_application_environment",
            application_id = application_id.0.to_string(),
            environment_name = environment_name.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_application_environment_internal(application_id.0, environment_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_application_environment_internal(
        &self,
        application_id: ApplicationId,
        environment_name: String,
        auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        let environment = self
            .environment_service
            .get_in_application(application_id, &EnvironmentName(environment_name), &auth)
            .await?;
        Ok(Json(environment))
    }

    /// List all application environments
    #[oai(
        path = "/apps/:application_id/envs",
        method = "get",
        operation_id = "list_application_environments",
        tag = ApiTags::Application
    )]
    pub async fn list_application_environments(
        &self,
        application_id: Path<ApplicationId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Environment>>> {
        let record = recorded_http_api_request!(
            "list_application_environments",
            application_id = application_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_application_environments_internal(application_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_application_environments_internal(
        &self,
        application_id: ApplicationId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<Environment>>> {
        let environments = self
            .environment_service
            .list_in_application(application_id, &auth)
            .await?;

        Ok(Json(Page {
            values: environments,
        }))
    }

    /// List all environments that are visible to the current user, either directly or through shares.
    #[oai(
        path = "/envs",
        method = "get",
        operation_id = "list_visible_environments"
    )]
    pub async fn list_visible_environments(
        &self,
        token: GolemSecurityScheme,
        account_email: Query<Option<AccountEmail>>,
        app_name: Query<Option<ApplicationName>>,
        env_name: Query<Option<EnvironmentName>>,
    ) -> ApiResult<Json<Page<EnvironmentWithDetails>>> {
        let record = recorded_http_api_request!("list_visible_environments",);

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_visible_environments_internal(account_email.0, app_name.0, env_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_visible_environments_internal(
        &self,
        account_email: Option<AccountEmail>,
        app_name: Option<ApplicationName>,
        env_name: Option<EnvironmentName>,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<EnvironmentWithDetails>>> {
        let environments = self
            .environment_service
            .list_visible_environments(
                account_email.as_ref(),
                app_name.as_ref(),
                env_name.as_ref(),
                &auth,
            )
            .await?;
        Ok(Json(Page {
            values: environments,
        }))
    }

    /// Get environment by id.
    #[oai(
        path = "/envs/:environment_id",
        method = "get",
        operation_id = "get_environment"
    )]
    pub async fn get_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "get_environment",
            environment_id = environment_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        let environment = self
            .environment_service
            .get(environment_id, false, &auth)
            .await?;
        Ok(Json(environment))
    }

    /// Update environment by id.
    #[oai(
        path = "/envs/:environment_id",
        method = "patch",
        operation_id = "update_environment"
    )]
    pub async fn update_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<EnvironmentUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "update_environment",
            environment_id = environment_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_environment_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_environment_internal(
        &self,
        environment_id: EnvironmentId,
        payload: EnvironmentUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        let result = self
            .environment_service
            .update(environment_id, payload, &auth)
            .await?;
        Ok(Json(result))
    }

    /// Delete environment by id.
    #[oai(
        path = "/envs/:environment_id",
        method = "delete",
        operation_id = "delete_environment"
    )]
    pub async fn delete_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        current_revision: Query<EnvironmentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        let record = recorded_http_api_request!(
            "delete_environment",
            environment_id = environment_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_environment_internal(environment_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_environment_internal(
        &self,
        environment_id: EnvironmentId,
        current_revision: EnvironmentRevision,
        auth: AuthCtx,
    ) -> ApiResult<NoContentResponse> {
        self.environment_service
            .delete(environment_id, current_revision, &auth)
            .await?;
        Ok(NoContentResponse::NoContent)
    }

    /// Get the current deployment plan
    #[oai(
        path = "/envs/:environment_id/plan",
        method = "get",
        operation_id = "get_environment_deployment_plan"
    )]
    async fn get_environment_deployment_plan(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<DeploymentPlan>> {
        let record = recorded_http_api_request!(
            "get_environment_deployment_plan",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_deployment_plan_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_deployment_plan_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<DeploymentPlan>> {
        let deployment_plan = self
            .deployment_service
            .get_current_deployment_plan(environment_id, &auth)
            .await?;
        Ok(Json(deployment_plan))
    }

    /// Rollback an environment to a previous deployment
    #[oai(
        path = "/envs/:environment_id/current-deployment",
        method = "put",
        operation_id = "rollback_environment",
        tag = ApiTags::Deployment
    )]
    async fn rollback_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<DeploymentRollback>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<CurrentDeployment>> {
        let record = recorded_http_api_request!(
            "rollback_environment",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .rollback_environment_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn rollback_environment_internal(
        &self,
        environment_id: EnvironmentId,
        payload: DeploymentRollback,
        auth: AuthCtx,
    ) -> ApiResult<Json<CurrentDeployment>> {
        let current_deployment = self
            .deployment_write_service
            .rollback_environment(environment_id, payload, &auth)
            .await?;

        Ok(Json(current_deployment))
    }

    /// List all deployments in this environment
    #[oai(
        path = "/envs/:environment_id/deployments",
        method = "get",
        operation_id = "list_deployments",
        tag = ApiTags::Deployment
    )]
    async fn list_deployments(
        &self,
        environment_id: Path<EnvironmentId>,
        version: Query<Option<DeploymentVersion>>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Deployment>>> {
        let record = recorded_http_api_request!(
            "list_deployments",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_deployments_internal(environment_id.0, version.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_deployments_internal(
        &self,
        environment_id: EnvironmentId,
        version: Option<DeploymentVersion>,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<Deployment>>> {
        let deployments = self
            .deployment_service
            .list_deployments(environment_id, version, &auth)
            .await?;
        Ok(Json(Page {
            values: deployments,
        }))
    }

    /// Deploy the current staging area of this environment
    #[oai(
        path = "/envs/:environment_id/deployments",
        method = "post",
        operation_id = "deploy_environment",
        tag = ApiTags::Deployment
    )]
    async fn deploy_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<DeploymentCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<CurrentDeployment>> {
        let record = recorded_http_api_request!(
            "deploy_environment",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .deploy_environment_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn deploy_environment_internal(
        &self,
        environment_id: EnvironmentId,
        payload: DeploymentCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<CurrentDeployment>> {
        let deployment = self
            .deployment_write_service
            .create_deployment(environment_id, payload, &auth)
            .await?;
        Ok(Json(deployment))
    }

    /// Get the deployment summary of a deployed deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_id/summary",
        method = "get",
        operation_id = "get_deployment_summary"
    )]
    async fn get_deployment_summary(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_id: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<DeploymentSummary>> {
        let record = recorded_http_api_request!(
            "get_deployment_summary",
            environment_id = environment_id.0.to_string(),
            deployment_id = deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployment_summary_internal(environment_id.0, deployment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_summary_internal(
        &self,
        environment_id: EnvironmentId,
        deployment_id: DeploymentRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<DeploymentSummary>> {
        let deployment_plan = self
            .deployment_service
            .get_deployment_summary(environment_id, deployment_id, &auth)
            .await?;
        Ok(Json(deployment_plan))
    }

    /// List all registered agent types in a deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_id/agent-types",
        method = "get",
        operation_id = "list_deployment_agent_types"
    )]
    async fn list_deployment_agent_types(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_id: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<DeployedRegisteredAgentType>>> {
        let record = recorded_http_api_request!(
            "list_deployment_agent_types",
            environment_id = environment_id.0.to_string(),
            deployment_id = deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_deployment_agent_types_internal(environment_id.0, deployment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_deployment_agent_types_internal(
        &self,
        environment_id: EnvironmentId,
        deployment_id: DeploymentRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<DeployedRegisteredAgentType>>> {
        let agent_types = self
            .deployment_service
            .list_deployment_agent_types(environment_id, deployment_id, &auth)
            .await?;
        Ok(Json(Page {
            values: agent_types,
        }))
    }

    /// Get a registered agent type in a deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_id/agent-types/:agent_type_name",
        method = "get",
        operation_id = "get_deployment_agent_type"
    )]
    async fn get_deployment_agent_type(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_id: Path<DeploymentRevision>,
        agent_type_name: Path<AgentTypeName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<DeployedRegisteredAgentType>> {
        let record = recorded_http_api_request!(
            "get_deployment_agent_type",
            environment_id = environment_id.0.to_string(),
            deployment_id = deployment_id.0.to_string(),
            agent_type_name = agent_type_name.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployment_agent_type_internal(
                environment_id.0,
                deployment_id.0,
                agent_type_name.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_agent_type_internal(
        &self,
        environment_id: EnvironmentId,
        deployment_id: DeploymentRevision,
        agent_type_name: AgentTypeName,
        auth: AuthCtx,
    ) -> ApiResult<Json<DeployedRegisteredAgentType>> {
        let agent_type = self
            .deployment_service
            .get_deployment_agent_type(environment_id, deployment_id, &agent_type_name, &auth)
            .await?;
        Ok(Json(agent_type))
    }
}
