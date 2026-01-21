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
use crate::services::http_api_deployment::HttpApiDeploymentService;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_deployment::{HttpApiDeployment, HttpApiDeploymentId};
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentCreation, HttpApiDeploymentRevision, HttpApiDeploymentUpdate,
};
use golem_common::model::poem::NoContentResponse;
use golem_common::model::{Page, UntypedJsonBody};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::{AuthCtx, GolemSecurityScheme};
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct HttpApiDeploymentsApi {
    http_api_deployment_service: Arc<HttpApiDeploymentService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::ApiDeployment
)]
#[allow(unused_variables)]
impl HttpApiDeploymentsApi {
    pub fn new(
        http_api_deployment_service: Arc<HttpApiDeploymentService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            http_api_deployment_service,
            auth_service,
        }
    }

    /// Create a new api-deployment in the environment
    #[oai(
        path = "/envs/:environment_id/http-api-deployments",
        method = "post",
        operation_id = "create_http_api_deployment",
        tag = ApiTags::Environment,
    )]
    async fn create_http_api_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<HttpApiDeploymentCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let record = recorded_http_api_request!(
            "create_http_api_deployment",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_http_api_deployment_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_http_api_deployment_internal(
        &self,
        environment_id: EnvironmentId,
        payload: HttpApiDeploymentCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let result = self
            .http_api_deployment_service
            .create(environment_id, payload, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Get an api-deployment by id
    #[oai(
        path = "/http-api-deployments/:http_api_deployment_id",
        method = "get",
        operation_id = "get_http_api_deployment"
    )]
    async fn get_http_api_deployment(
        &self,
        http_api_deployment_id: Path<HttpApiDeploymentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let record = recorded_http_api_request!(
            "get_http_api_deployment",
            http_api_deployment_id = http_api_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_http_api_deployment_internal(http_api_deployment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_http_api_deployment_internal(
        &self,
        http_api_deployment_id: HttpApiDeploymentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let http_api_deployment = self
            .http_api_deployment_service
            .get_staged(http_api_deployment_id, &auth)
            .await?;

        Ok(Json(http_api_deployment))
    }

    /// Update an api-deployment
    #[oai(
        path = "/http-api-deployments/:http_api_deployment_id",
        method = "patch",
        operation_id = "update_http_api_deployment"
    )]
    async fn update_http_api_deployment(
        &self,
        http_api_deployment_id: Path<HttpApiDeploymentId>,
        payload: Json<HttpApiDeploymentUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let record = recorded_http_api_request!(
            "update_http_api_deployment",
            http_api_deployment_id = http_api_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_http_api_deployment_internal(http_api_deployment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_http_api_deployment_internal(
        &self,
        http_api_deployment_id: HttpApiDeploymentId,
        payload: HttpApiDeploymentUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let updated = self
            .http_api_deployment_service
            .update(http_api_deployment_id, payload, &auth)
            .await?;

        Ok(Json(updated))
    }

    /// Delete an api-deployment
    #[oai(
        path = "/http-api-deployments/:http_api_deployment_id",
        method = "delete",
        operation_id = "delete_http_api_deployment"
    )]
    async fn delete_http_api_deployment(
        &self,
        http_api_deployment_id: Path<HttpApiDeploymentId>,
        current_revision: Query<HttpApiDeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        let record = recorded_http_api_request!(
            "delete_http_api_deployment",
            http_api_deployment_id = http_api_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_http_api_deployment_internal(http_api_deployment_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_http_api_deployment_internal(
        &self,
        http_api_deployment_id: HttpApiDeploymentId,
        current_revision: HttpApiDeploymentRevision,
        auth: AuthCtx,
    ) -> ApiResult<NoContentResponse> {
        self.http_api_deployment_service
            .delete(http_api_deployment_id, current_revision, &auth)
            .await?;

        Ok(NoContentResponse::NoContent)
    }

    /// Get a specific http api deployment revision
    #[oai(
        path = "/http-api-deployment/:http_api_deployment_id/revisions/:revision",
        method = "get",
        operation_id = "get_http_api_deployment_revision"
    )]
    async fn get_http_api_deployment_revision(
        &self,
        http_api_deployment_id: Path<HttpApiDeploymentId>,
        revision: Path<HttpApiDeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let record = recorded_http_api_request!(
            "get_http_api_deployment_revision",
            http_api_deployment_id = http_api_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_http_api_deployment_revision_internal(http_api_deployment_id.0, revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_http_api_deployment_revision_internal(
        &self,
        http_api_deployment_id: HttpApiDeploymentId,
        revision: HttpApiDeploymentRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let result = self
            .http_api_deployment_service
            .get_revision(http_api_deployment_id, revision, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Get http api deployment by domain in the environment
    #[oai(
        path = "/envs/:environment_id/http-api-deployments/:domain",
        method = "get",
        operation_id = "get_http_api_deployment_in_environment",
        tag = ApiTags::Environment
    )]
    async fn get_http_api_deployment_in_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        domain: Path<Domain>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let record = recorded_http_api_request!(
            "get_http_api_definition_in_environment",
            environment_id = environment_id.0.to_string(),
            domain = domain.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_http_api_deployment_in_environment_internal(environment_id.0, domain.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_http_api_deployment_in_environment_internal(
        &self,
        environment_id: EnvironmentId,
        domain: Domain,
        auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let http_api_definition = self
            .http_api_deployment_service
            .get_staged_by_domain(environment_id, &domain, &auth)
            .await?;

        Ok(Json(http_api_definition))
    }

    /// Get http api deployment by domain in the deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/http-api-deployments/:domain",
        method = "get",
        operation_id = "get_http_api_deployment_in_deployment",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment,
    )]
    async fn get_http_api_deployment_in_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        domain: Path<Domain>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let record = recorded_http_api_request!(
            "get_http_api_deployment_in_deployment",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
            domain = domain.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_http_api_deployment_in_deployment_internal(
                environment_id.0,
                deployment_revision.0,
                domain.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_http_api_deployment_in_deployment_internal(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        domain: Domain,
        auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        let http_api_definition = self
            .http_api_deployment_service
            .get_in_deployment_by_domain(environment_id, deployment_revision, &domain, &auth)
            .await?;

        Ok(Json(http_api_definition))
    }

    /// Get openapi spec of http api definition in the deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/http-api-deployments/:domain/openapi",
        method = "get",
        operation_id = "get_openapi_of_http_api_deployment_in_deployment",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment,
    )]
    async fn get_openapi_of_http_api_deployment_in_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        domain: Path<Domain>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<UntypedJsonBody>> {
        unimplemented!()
    }

    /// List http api deployment by domain in the environment
    #[oai(
        path = "/envs/:environment_id/http-api-deployments",
        method = "get",
        operation_id = "list_http_api_deployments_in_environment",
        tag = ApiTags::Environment
    )]
    async fn list_http_api_deployments_in_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<HttpApiDeployment>>> {
        let record = recorded_http_api_request!(
            "list_http_api_deployments_in_environment",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_http_api_deployments_in_environment_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_http_api_deployments_in_environment_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<HttpApiDeployment>>> {
        let values = self
            .http_api_deployment_service
            .list_staged(environment_id, &auth)
            .await?;

        Ok(Json(Page { values }))
    }

    /// Get http api deployment by domain in the deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/http-api-deployments",
        method = "get",
        operation_id = "list_http_api_deployments_in_deployment",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment,
    )]
    async fn list_http_api_deployments_in_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<HttpApiDeployment>>> {
        let record = recorded_http_api_request!(
            "list_http_api_deployments_in_deployment",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_http_api_deployments_in_deployment_internal(
                environment_id.0,
                deployment_revision.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_http_api_deployments_in_deployment_internal(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<HttpApiDeployment>>> {
        let values = self
            .http_api_deployment_service
            .list_in_deployment(environment_id, deployment_revision, &auth)
            .await?;

        Ok(Json(Page { values }))
    }
}
