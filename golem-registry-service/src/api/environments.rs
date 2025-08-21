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
use golem_common::api::environment::{DeployEnvironmentRequest, UpdateEnvironmentRequest};
use golem_common::model::auth::AuthCtx;
use golem_common::model::deployment::Deployment;
use golem_common::model::environment::*;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use tracing::Instrument;
use crate::services::environment::EnvironmentService;
use std::sync::Arc;
use golem_common::model::environment_share::{EnvironmentShare, NewEnvironmentShare};
use crate::services::environment_share::EnvironmentShareService;
use golem_common::model::account::AccountId;
use uuid::Uuid;

pub struct EnvironmentsApi {
    environment_service: Arc<EnvironmentService>,
    environment_share_service: Arc<EnvironmentShareService>
}

#[OpenApi(
    prefix_path = "/v1/envs",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Environment
)]
impl EnvironmentsApi {
    pub fn new(
        environment_service: Arc<EnvironmentService>,
        environment_share_service: Arc<EnvironmentShareService>
    ) -> Self {
        Self {
            environment_service,
            environment_share_service
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

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_environment_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_internal(
        &self,
        environment_id: EnvironmentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        let environment = self.environment_service.get(&environment_id).await?;
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
        payload: Json<UpdateEnvironmentRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "update_environment",
            environment_id = environment_id.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .update_environment_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_environment_internal(
        &self,
        _application_id: EnvironmentId,
        _payload: UpdateEnvironmentRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        todo!()
    }

    /// Get hash of the currently deployed environment
    #[oai(
        path = "/:environment_id/current-deployment/hash",
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
        path = "/:environment_id/current-deployment/summary",
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

    /// Get the current deployment plan. This is equivalent to a summary of the current staging area.
    #[oai(
        path = "/:environment_id/plan",
        method = "get",
        operation_id = "get_environment_deployment_plan"
    )]
    async fn get_environment_deployment_plan(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentSummary>> {
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
    ) -> ApiResult<Json<EnvironmentSummary>> {
        todo!()
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

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_deployments_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_deployments_internal(
        &self,
        _environment_id: EnvironmentId,
        _token: AuthCtx,
    ) -> ApiResult<Json<Page<Deployment>>> {
        todo!()
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
        payload: Json<DeployEnvironmentRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Deployment>> {
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
    ) -> ApiResult<Json<Deployment>> {
        todo!()
    }

    /// Deploy the current staging area of this environment
    #[oai(
        path = "/:environment_id/shares",
        method = "post",
        operation_id = "create_environment_share",
        tag = ApiTags::EnvironmentShares
    )]
    async fn create_environment_share(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<NewEnvironmentShare>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentShare>> {
        let record = recorded_http_api_request!(
            "create_environment_share",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .create_environment_share_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_environment_share_internal(
        &self,
        environment_id: EnvironmentId,
        payload: NewEnvironmentShare,
        _token: AuthCtx,
    ) -> ApiResult<Json<EnvironmentShare>> {
        let actor = AccountId(Uuid::new_v4());

        let result = self.environment_share_service
            .create(environment_id, payload, actor)
            .await?;

        Ok(Json(result))
    }

    /// Deploy the current staging area of this environment
    #[oai(
        path = "/:environment_id/shares",
        method = "get",
        operation_id = "get_environment_shares",
        tag = ApiTags::EnvironmentShares
    )]
    async fn get_environment_shares(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<EnvironmentShare>>> {
        let record = recorded_http_api_request!(
            "get_environment_shares",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_environment_shares_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_shares_internal(
        &self,
        environment_id: EnvironmentId,
        _token: AuthCtx,
    ) -> ApiResult<Json<Page<EnvironmentShare>>> {
        let result = self.environment_share_service
            .get_shares_in_environment(environment_id)
            .await?;

        Ok(Json(Page { values: result }))
    }
}
