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
use crate::services::deployment::DeploymentService;
use crate::services::environment::EnvironmentService;
use crate::services::environment_plugin_grant::EnvironmentPluginGrantService;
use crate::services::environment_share::EnvironmentShareService;
use golem_common::api::Page;
use golem_common::model::account::AccountId;
use golem_common::model::deployment::{
    Deployment, DeploymentCreation, DeploymentPlan, DeploymentRevision, DeploymentSummary,
};
use golem_common::model::environment::*;
use golem_common::model::environment_plugin_grant::{
    EnvironmentPluginGrant, EnvironmentPluginGrantCreation,
};
use golem_common::model::environment_share::{EnvironmentShare, EnvironmentShareCreation};
use golem_common::model::poem::NoContentResponse;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;
use uuid::Uuid;

pub struct EnvironmentsApi {
    environment_service: Arc<EnvironmentService>,
    deployment_service: Arc<DeploymentService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/envs",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Environment
)]
impl EnvironmentsApi {
    pub fn new(
        environment_service: Arc<EnvironmentService>,
        deployment_service: Arc<DeploymentService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            environment_service,
            deployment_service,
            auth_service,
        }
    }

    /// Get environment by id.
    #[oai(
        path = "/:environment_id",
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
            .get(&environment_id, false, &auth)
            .await?;
        Ok(Json(environment))
    }

    /// Update environment by id.
    #[oai(
        path = "/:environment_id",
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
        path = "/:environment_id",
        method = "delete",
        operation_id = "delete_environment"
    )]
    pub async fn delete_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        let record = recorded_http_api_request!(
            "delete_environment",
            environment_id = environment_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_environment_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_environment_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<NoContentResponse> {
        self.environment_service
            .delete(environment_id, &auth)
            .await?;
        Ok(NoContentResponse::NoContent)
    }

    /// Get the current deployment plan
    #[oai(
        path = "/:environment_id/plan",
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
            .get_current_deployment_plan(&environment_id, &auth)
            .await?;
        Ok(Json(deployment_plan))
    }

    /// Get all deployments in this environment
    #[oai(
        path = "/:environment_id/deployments",
        method = "get",
        operation_id = "get_deployments",
        tag = ApiTags::Deployment
    )]
    async fn get_deployments(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Deployment>>> {
        let record = recorded_http_api_request!(
            "get_deployments",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployments_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployments_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<Deployment>>> {
        let deployments = self
            .deployment_service
            .list_deployments(&environment_id, &auth)
            .await?;
        Ok(Json(Page {
            values: deployments,
        }))
    }

    /// Deploy the current staging area of this environment
    #[oai(
        path = "/:environment_id/deployments",
        method = "post",
        operation_id = "deploy_environment",
        tag = ApiTags::Deployment
    )]
    async fn deploy_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<DeploymentCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Deployment>> {
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
    ) -> ApiResult<Json<Deployment>> {
        let deployment = self
            .deployment_service
            .create_deployment(&environment_id, payload, &auth)
            .await?;
        Ok(Json(deployment))
    }

    /// Get the deployment plan of a deployed deployment
    #[oai(
        path = "/:environment_id/deployments/:deployment_id/plan",
        method = "get",
        operation_id = "get_environment_deployed_deployment_plan"
    )]
    async fn get_environment_deployed_deployment_plan(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_id: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<DeploymentSummary>> {
        let record = recorded_http_api_request!(
            "get_environment_deployed_deployment_plan",
            environment_id = environment_id.0.to_string(),
            deployment_id = deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_deployed_deployment_plan_internal(
                environment_id.0,
                deployment_id.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_deployed_deployment_plan_internal(
        &self,
        environment_id: EnvironmentId,
        deployment_id: DeploymentRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<DeploymentSummary>> {
        let deployment_plan = self
            .deployment_service
            .get_deployed_deployment_summary(&environment_id, deployment_id, &auth)
            .await?;
        Ok(Json(deployment_plan))
    }
}
