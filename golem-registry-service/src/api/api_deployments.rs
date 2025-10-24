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
use golem_common::api::{Page, UpdateApiDeploymentRequest};
use golem_common::model::api_deployment::ApiDeploymentRevision;
use golem_common::model::api_deployment::{ApiDeployment, ApiDeploymentId};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct ApiDeploymentsApi {
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/api-deployments",
    tag = ApiTags::RegistryService,
    tag = ApiTags::ApiDeployment
)]
impl ApiDeploymentsApi {
    pub fn new(auth_service: Arc<AuthService>) -> Self {
        Self { auth_service }
    }

    /// Get an api-deployment by id
    #[oai(
        path = "/:api_deployment_id",
        method = "get",
        operation_id = "get_api_deployment"
    )]
    async fn get_api_deployment(
        &self,
        api_deployment_id: Path<ApiDeploymentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDeployment>> {
        let record = recorded_http_api_request!(
            "get_api_deployment",
            api_deployment_id = api_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_api_deployment_internal(api_deployment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_api_deployment_internal(
        &self,
        _api_deployment_id: ApiDeploymentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDeployment>> {
        todo!()
    }

    /// Get all revisions for an api-deployment
    #[oai(
        path = "/:api_deployment_id/revisions",
        method = "get",
        operation_id = "get_api_deployment_revisions"
    )]
    async fn get_api_deployment_revisions(
        &self,
        api_deployment_id: Path<ApiDeploymentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ApiDeployment>>> {
        let record = recorded_http_api_request!(
            "get_api_deployment_revisions",
            api_deployment_id = api_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_api_deployment_revisions_internal(api_deployment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_api_deployment_revisions_internal(
        &self,
        _api_deployment_id: ApiDeploymentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<ApiDeployment>>> {
        todo!()
    }

    /// Get specific revision an api-deployment
    #[oai(
        path = "/:api_deployment_id/revisions/:revision",
        method = "get",
        operation_id = "get_api_deployment_revision"
    )]
    async fn get_api_deployment_revision(
        &self,
        api_deployment_id: Path<ApiDeploymentId>,
        revision: Path<ApiDeploymentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDeployment>> {
        let record = recorded_http_api_request!(
            "get_api_deployment_revision",
            api_deployment_id = api_deployment_id.0.to_string(),
            revision = revision.0.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_api_deployment_revision_internal(api_deployment_id.0, revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_api_deployment_revision_internal(
        &self,
        _api_deployment_id: ApiDeploymentId,
        _revision: ApiDeploymentRevision,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDeployment>> {
        todo!()
    }

    /// Update an api-deployment
    #[oai(
        path = "/:api_deployment_id",
        method = "patch",
        operation_id = "update_api_deployment"
    )]
    async fn update_api_deployment(
        &self,
        api_deployment_id: Path<ApiDeploymentId>,
        payload: Json<UpdateApiDeploymentRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ApiDeployment>> {
        let record = recorded_http_api_request!(
            "update_api_deployment",
            api_deployment_id = api_deployment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_api_deployment_internal(api_deployment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_api_deployment_internal(
        &self,
        _api_deployment_id: ApiDeploymentId,
        _payload: UpdateApiDeploymentRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<ApiDeployment>> {
        todo!()
    }
}
