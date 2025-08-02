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
use super::model::{CreateComponentRequest, UpdateComponentRequest};
use golem_common_next::model::component::{Component, ComponentName};
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use poem_openapi::payload::{Binary, Json};
use poem_openapi::*;
use tracing::Instrument;
use poem_openapi::param::Path;
use golem_common_next::model::EnvironmentId;
use golem_common_next::model::auth::AuthCtx;
use golem_common_next::api::Page;
use poem::Body;
use golem_common_next::model::api_deployment::{ApiDeployment, ApiSite};

pub struct EnvironmentApiDeploymentsApi {}

#[OpenApi(prefix_path = "/v1/envs/:environment_id/api-deployments", tag = ApiTags::Component)]
impl EnvironmentApiDeploymentsApi {
    /// Get all api-deployments in the environment
    #[oai(
        path = "/",
        method = "get",
        operation_id = "get_api_deployments"
    )]
    async fn get_api_deployments(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ApiDeployment>>> {
        let record = recorded_http_api_request!(
            "get_api_deployments",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_api_deployments_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_api_deployments_internal(
        &self,
        _environment_id: EnvironmentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<ApiDeployment>>> {
        todo!()
    }

    /// Get api-deployment by site
    #[oai(
        path = "/:site",
        method = "get",
        operation_id = "get_api_deployment"
    )]
    async fn get_api_deployment(
        &self,
        environment_id: Path<EnvironmentId>,
        site: Path<ApiSite>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDeployment>> {
        let record = recorded_http_api_request!(
            "get_api_deployments",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_api_deployment_internal(environment_id.0, site.0,  auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_api_deployment_internal(
        &self,
        _environment_id: EnvironmentId,
        _site: ApiSite,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDeployment>> {
        todo!()
    }
}
