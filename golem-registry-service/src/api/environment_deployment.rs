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
use golem_common_next::api::environment::{DeployEnvironmentRequest, DeployEnvironmentResponse};
use golem_common_next::model::auth::AuthCtx;
use golem_common_next::model::environment::{
    EnvironmentDeploymentPlan, EnvironmentHash, EnvironmentId, EnvironmentSummary,
};
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use tracing::Instrument;

pub struct EnvironmentDeploymentApi {}

#[OpenApi(prefix_path = "/v1/envs", tag = ApiTags::Environment)]
impl EnvironmentDeploymentApi {
    /// Get hash of the currently deployed environment
    #[oai(
        path = "/:environment_id/deployed/hash",
        method = "get",
        operation_id = "get_deployed_environment_hash"
    )]
    async fn get_deployed_environment_hash(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentHash>> {
        let record = recorded_http_api_request!(
            "get_deployed_environment_hash",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_deployed_environment_hash_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployed_environment_hash_internal(
        &self,
        _environment_id: EnvironmentId,
        _token: AuthCtx,
    ) -> ApiResult<Json<EnvironmentHash>> {
        todo!()
    }

    /// Get summary of the currently deployed environment
    #[oai(
        path = "/:environment_id/deployed/summary",
        method = "get",
        operation_id = "get_deployed_environment_summary"
    )]
    async fn get_deployed_environment_summary(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentSummary>> {
        let record = recorded_http_api_request!(
            "get_deployed_environment_summary",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_deployed_environment_summary_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployed_environment_summary_internal(
        &self,
        _environment_id: EnvironmentId,
        _token: AuthCtx,
    ) -> ApiResult<Json<EnvironmentSummary>> {
        todo!()
    }

    /// Get plan of the deployment that would be performed given the current staging area
    #[oai(
        path = "/:environment_id/plan",
        method = "get",
        operation_id = "get_environment_deployment_plan"
    )]
    async fn get_environment_deployment_plan(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentDeploymentPlan>> {
        let record = recorded_http_api_request!(
            "get_environment_deployment_plan",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_environment_deployment_plan_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_deployment_plan_internal(
        &self,
        _environment_id: EnvironmentId,
        _token: AuthCtx,
    ) -> ApiResult<Json<EnvironmentDeploymentPlan>> {
        todo!()
    }

    /// Deploy the current staging area of this environment
    #[oai(
        path = "/:environment_id/deployments",
        method = "post",
        operation_id = "deploy_environment"
    )]
    async fn deploy_environment(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<DeployEnvironmentRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<DeployEnvironmentResponse>> {
        let record = recorded_http_api_request!(
            "deploy_environment",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .deploy_environment_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn deploy_environment_internal(
        &self,
        _environment_id: EnvironmentId,
        _payload: DeployEnvironmentRequest,
        _token: AuthCtx,
    ) -> ApiResult<Json<DeployEnvironmentResponse>> {
        todo!()
    }
}
