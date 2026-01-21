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
use golem_common::model::Page;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_deployment::HttpApiDeploymentRevision;
use golem_common::model::http_api_deployment::{HttpApiDeployment, HttpApiDeploymentId};
use golem_common::model::http_api_deployment_legacy::{
    LegacyHttpApiDeployment, LegacyHttpApiDeploymentCreation, LegacyHttpApiDeploymentUpdate,
};
use golem_common::model::poem::NoContentResponse;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;

pub struct LegacyHttpApiDeploymentsApi;

#[OpenApi(
    prefix_path = "/stubs",
    tag = ApiTags::RegistryService,
    tag = ApiTags::ApiDeployment
)]
#[allow(unused_variables)]
impl LegacyHttpApiDeploymentsApi {
    /// Create a new api-deployment in the environment
    #[oai(
        path = "/envs/:environment_id/http-api-deployments",
        method = "post",
        operation_id = "create_http_api_deployment_legacy",
        tag = ApiTags::Environment,
    )]
    async fn create_http_api_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<LegacyHttpApiDeploymentCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDeployment>> {
        unimplemented!()
    }

    /// Get an api-deployment by id
    #[oai(
        path = "/http-api-deployments/:http_api_deployment_id",
        method = "get",
        operation_id = "get_http_api_deployment_legacy"
    )]
    async fn get_http_api_deployment(
        &self,
        http_api_deployment_id: Path<HttpApiDeploymentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<LegacyHttpApiDeployment>> {
        unimplemented!()
    }

    /// Update an api-deployment
    #[oai(
        path = "/http-api-deployments/:http_api_deployment_id",
        method = "patch",
        operation_id = "update_http_api_deployment_legacy"
    )]
    async fn update_http_api_deployment(
        &self,
        http_api_deployment_id: Path<HttpApiDeploymentId>,
        payload: Json<LegacyHttpApiDeploymentUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<LegacyHttpApiDeployment>> {
        unimplemented!()
    }

    /// Delete an api-deployment
    #[oai(
        path = "/http-api-deployments/:http_api_deployment_id",
        method = "delete",
        operation_id = "delete_http_api_deployment_legacy"
    )]
    async fn delete_http_api_deployment(
        &self,
        http_api_deployment_id: Path<HttpApiDeploymentId>,
        current_revision: Query<HttpApiDeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        unimplemented!()
    }

    /// Get a specific http api deployment revision
    #[oai(
        path = "/http-api-deployment/:http_api_deployment_id/revisions/:revision",
        method = "get",
        operation_id = "get_http_api_deployment_revision_legacy"
    )]
    async fn get_http_api_deployment_revision(
        &self,
        http_api_deployment_id: Path<HttpApiDeploymentId>,
        revision: Path<HttpApiDeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<LegacyHttpApiDeployment>> {
        unimplemented!()
    }

    /// Get http api deployment by domain in the environment
    #[oai(
        path = "/envs/:environment_id/http-api-deployments/:domain",
        method = "get",
        operation_id = "get_http_api_deployment_in_environment_legacy",
        tag = ApiTags::Environment
    )]
    async fn get_http_api_deployment_in_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        domain: Path<Domain>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<LegacyHttpApiDeployment>> {
        unimplemented!()
    }

    /// Get http api deployment by domain in the deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/http-api-deployments/:domain",
        method = "get",
        operation_id = "get_http_api_deployment_in_deployment_legacy",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment,
    )]
    async fn get_http_api_deployment_in_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        domain: Path<Domain>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<LegacyHttpApiDeployment>> {
        unimplemented!()
    }

    /// List http api deployment by domain in the environment
    #[oai(
        path = "/envs/:environment_id/http-api-deployments",
        method = "get",
        operation_id = "list_http_api_deployments_in_environment_legacy",
        tag = ApiTags::Environment
    )]
    async fn list_http_api_deployments_in_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<LegacyHttpApiDeployment>>> {
        unimplemented!()
    }

    /// Get http api deployment by domain in the deployment
    #[oai(
        path = "/envs/:environment_id/deployments/:deployment_revision/http-api-deployments",
        method = "get",
        operation_id = "list_http_api_deployments_in_deployment_legacy",
        tag = ApiTags::Environment,
        tag = ApiTags::Deployment,
    )]
    async fn list_http_api_deployments_in_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<LegacyHttpApiDeployment>>> {
        unimplemented!()
    }
}
