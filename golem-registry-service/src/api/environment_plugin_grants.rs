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
use crate::services::environment_plugin_grant::EnvironmentPluginGrantService;
use golem_common::model::Page;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_plugin_grant::{
    EnvironmentPluginGrant, EnvironmentPluginGrantCreation, EnvironmentPluginGrantId,
};
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

pub struct EnvironmentPluginGrantsApi {
    environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::EnvironmentPluginGrants
)]
impl EnvironmentPluginGrantsApi {
    pub fn new(
        environment_plugin_grant_service: Arc<EnvironmentPluginGrantService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            environment_plugin_grant_service,
            auth_service,
        }
    }

    /// Create a new environment plugin grant
    #[oai(
        path = "/envs/:environment_id/plugins",
        method = "post",
        operation_id = "create_environment_plugin_grant",
        tag = ApiTags::Environment
    )]
    pub async fn create_environment_plugin_grant(
        &self,
        environment_id: Path<EnvironmentId>,
        data: Json<EnvironmentPluginGrantCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentPluginGrant>> {
        let record = recorded_http_api_request!(
            "create_environment_plugin_grant",
            environment_id = environment_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_environment_plugin_grant_internal(environment_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_environment_plugin_grant_internal(
        &self,
        environment_id: EnvironmentId,
        data: EnvironmentPluginGrantCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<EnvironmentPluginGrant>> {
        let grant = self
            .environment_plugin_grant_service
            .create(environment_id, data, &auth)
            .await?;
        Ok(Json(grant))
    }

    /// List all environment plugin grants in the environment
    #[oai(
        path = "/envs/:environment_id/plugins",
        method = "get",
        operation_id = "list_environment_plugin_grants",
        tag = ApiTags::Environment
    )]
    pub async fn list_environment_plugin_grants(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<EnvironmentPluginGrant>>> {
        let record = recorded_http_api_request!(
            "list_environment_plugin_grants_internal",
            environment_id = environment_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_environment_plugin_grants_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_environment_plugin_grants_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<EnvironmentPluginGrant>>> {
        let grants = self
            .environment_plugin_grant_service
            .list_in_environment(&environment_id, &auth)
            .await?;

        Ok(Json(Page { values: grants }))
    }

    /// Get environment plugin grant by id
    #[oai(
        path = "/environment-plugins/:environment_plugin_grant_id",
        method = "get",
        operation_id = "get_environment_plugin_grant",
        tag = ApiTags::EnvironmentPluginGrants
    )]
    pub async fn get_environment_plugin_grant(
        &self,
        environment_plugin_grant_id: Path<EnvironmentPluginGrantId>,
        #[oai(default)] include_deleted: Query<bool>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentPluginGrant>> {
        let record = recorded_http_api_request!(
            "get_environment_plugin_grant",
            environment_plugin_grant_id = environment_plugin_grant_id.0.to_string(),
            include_deleted = %include_deleted.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_plugin_grant_internal(
                environment_plugin_grant_id.0,
                include_deleted.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_plugin_grant_internal(
        &self,
        environment_plugin_grant_id: EnvironmentPluginGrantId,
        include_deleted: bool,
        auth: AuthCtx,
    ) -> ApiResult<Json<EnvironmentPluginGrant>> {
        let grant = self
            .environment_plugin_grant_service
            .get_by_id(&environment_plugin_grant_id, include_deleted, &auth)
            .await?;

        Ok(Json(grant))
    }

    /// Delete environment plugin grant
    #[oai(
        path = "/environment-plugins/:environment_plugin_grant_id",
        method = "delete",
        operation_id = "delete_environment_plugin_grant",
        tag = ApiTags::EnvironmentPluginGrants
    )]
    pub async fn delete_environment_plugin_grant(
        &self,
        environment_plugin_grant_id: Path<EnvironmentPluginGrantId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        let record = recorded_http_api_request!(
            "delete_environment_plugin_grant",
            environment_plugin_grant_id = environment_plugin_grant_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_environment_plugin_grant_internal(environment_plugin_grant_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_environment_plugin_grant_internal(
        &self,
        environment_plugin_grant_id: EnvironmentPluginGrantId,
        auth: AuthCtx,
    ) -> ApiResult<NoContentResponse> {
        self.environment_plugin_grant_service
            .delete(&environment_plugin_grant_id, &auth)
            .await?;

        Ok(NoContentResponse::NoContent)
    }
}
