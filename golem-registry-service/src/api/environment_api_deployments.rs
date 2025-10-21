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
use crate::model::auth::AuthCtx;
use crate::services::auth::AuthService;
use golem_common::api::{CreateApiDeploymentRequest, Page};
use golem_common::model::api_deployment::{ApiDeployment, ApiSiteString};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct EnvironmentApiDeploymentsApi {
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/envs",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Environment,
    tag = ApiTags::ApiDeployment
)]
impl EnvironmentApiDeploymentsApi {
    pub fn new(auth_service: Arc<AuthService>) -> Self {
        Self { auth_service }
    }

    /// Create a new api deployment
    #[oai(
        path = "/:environment_id/api-deployments",
        method = "post",
        operation_id = "create_api_deployment"
    )]
    async fn create_api_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<CreateApiDeploymentRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDeployment>> {
        let record = recorded_http_api_request!(
            "create_api_deployment",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_api_deployment_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_api_deployment_internal(
        &self,
        _environment_id: EnvironmentId,
        _payload: CreateApiDeploymentRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDeployment>> {
        todo!()
    }

    /// Get all api-deployments in the environment
    #[oai(
        path = "/:environment_id/api-deployments",
        method = "get",
        operation_id = "get_environment_api_deployments"
    )]
    async fn get_environment_api_deployments(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ApiDeployment>>> {
        let record = recorded_http_api_request!(
            "get_environment_api_deployments",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_api_deployments_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_api_deployments_internal(
        &self,
        _environment_id: EnvironmentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<ApiDeployment>>> {
        todo!()
    }

    /// Get api-deployment by site
    #[oai(
        path = "/:environment_id/api-deployments/:site",
        method = "get",
        operation_id = "get_environment_api_deployment"
    )]
    async fn get_environment_api_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        site: Path<ApiSiteString>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDeployment>> {
        let record = recorded_http_api_request!(
            "get_environment_api_deployment",
            environment_id = environment_id.0.to_string(),
            site = site.0.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_api_deployment_internal(environment_id.0, site.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_api_deployment_internal(
        &self,
        _environment_id: EnvironmentId,
        _site: ApiSiteString,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDeployment>> {
        todo!()
    }

    /// Get all api-deployments in a specific deployment
    #[oai(
        path = "/:environment_id/deployments/:deployment_revision_id/api-deployments",
        method = "get",
        operation_id = "get_deployment_api_deployments",
        tag = ApiTags::Deployment
    )]
    async fn get_deployment_api_deployments(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision_id: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ApiDeployment>>> {
        let record = recorded_http_api_request!(
            "get_deployment_api_deployments",
            environment_id = environment_id.0.to_string(),
            deployment_revision_id = deployment_revision_id.0.0,
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployment_api_deployments_internal(
                environment_id.0,
                deployment_revision_id.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_api_deployments_internal(
        &self,
        _environment_id: EnvironmentId,
        _deployment_revision_id: DeploymentRevision,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<ApiDeployment>>> {
        todo!()
    }

    /// Get api-deployment in a deployment by site
    #[oai(
        path = "/:environment_id/deployments/:deployment_revision_id/api-deployments/:site",
        method = "get",
        operation_id = "get_deployment_api_deployment",
        tag = ApiTags::Deployment
    )]
    async fn get_deployment_api_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision_id: Path<DeploymentRevision>,
        site: Path<ApiSiteString>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDeployment>> {
        let record = recorded_http_api_request!(
            "get_deployment_api_deployment",
            environment_id = environment_id.0.to_string(),
            deployment_revision_id = deployment_revision_id.0.0,
            site = site.0.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_deployment_api_deployment_internal(
                environment_id.0,
                deployment_revision_id.0,
                site.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_api_deployment_internal(
        &self,
        _environment_id: EnvironmentId,
        _deployment_revision_id: DeploymentRevision,
        _site: ApiSiteString,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDeployment>> {
        todo!()
    }
}
