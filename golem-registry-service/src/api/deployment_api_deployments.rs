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
use golem_common::api::Page;
use golem_common::model::api_deployment::{ApiDeployment, ApiSiteString};
use golem_common::model::auth::AuthCtx;
use golem_common::model::deployment::DeploymentId;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct DeploymentApiDeploymentsApi {}

#[OpenApi(prefix_path = "/v1/deployments", tag = ApiTags::Deployment, tag = ApiTags::ApiDeployment)]
impl DeploymentApiDeploymentsApi {
    /// Get all api-deployments in a specific deployment
    #[oai(
        path = "/:deployment_id/api-deployments",
        method = "get",
        operation_id = "get_deployment_api_deployments"
    )]
    async fn get_deployment_api_deployments(
        &self,
        deployment_id: Path<DeploymentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ApiDeployment>>> {
        let record = recorded_http_api_request!(
            "get_deployment_api_deployments",
            deployment_id = deployment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_deployment_api_deployments_internal(deployment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_api_deployments_internal(
        &self,
        _deployment_id: DeploymentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<ApiDeployment>>> {
        todo!()
    }

    /// Get api-deployment in a deployment by site
    #[oai(
        path = "/:deployment_id/api-deployments/:site",
        method = "get",
        operation_id = "get_deployment_api_deployment"
    )]
    async fn get_deployment_api_deployment(
        &self,
        deployment_id: Path<DeploymentId>,
        site: Path<ApiSiteString>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDeployment>> {
        let record = recorded_http_api_request!(
            "get_deployment_api_deployment",
            deployment_id = deployment_id.0.to_string(),
            site = site.0.0
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_deployment_api_deployment_internal(deployment_id.0, site.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployment_api_deployment_internal(
        &self,
        _deployment_id: DeploymentId,
        _site: ApiSiteString,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDeployment>> {
        todo!()
    }
}
