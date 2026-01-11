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
use crate::services::security_scheme::SecuritySchemeService;
use golem_common::model::Page;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::security_scheme::{
    SecuritySchemeCreation, SecuritySchemeDto, SecuritySchemeId, SecuritySchemeRevision,
    SecuritySchemeUpdate,
};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct SecuritySchemesApi {
    security_scheme_service: Arc<SecuritySchemeService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::ApiSecurity
)]
impl SecuritySchemesApi {
    pub fn new(
        security_scheme_service: Arc<SecuritySchemeService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            security_scheme_service,
            auth_service,
        }
    }

    /// Create a new security scheme
    #[oai(
        path = "/envs/:environment_id/security-schemes",
        method = "post",
        operation_id = "create_security_scheme",
        tag = ApiTags::Environment
    )]
    async fn create_security_scheme(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<SecuritySchemeCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<SecuritySchemeDto>> {
        let record = recorded_http_api_request!(
            "create_security_scheme",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_security_scheme_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_security_scheme_internal(
        &self,
        environment_id: EnvironmentId,
        payload: SecuritySchemeCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<SecuritySchemeDto>> {
        let result = self
            .security_scheme_service
            .create(environment_id, payload, &auth)
            .await?;

        Ok(Json(result.into()))
    }

    /// Get all security-schemes of the environment
    #[oai(
        path = "/envs/:environment_id/security-schemes",
        method = "get",
        operation_id = "get_environment_security_schemes",
        tag = ApiTags::Environment
    )]
    async fn get_environment_security_schemes(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<SecuritySchemeDto>>> {
        let record = recorded_http_api_request!(
            "get_environment_security_schemes",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_security_schemes_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_security_schemes_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<SecuritySchemeDto>>> {
        let result = self
            .security_scheme_service
            .get_security_schemes_in_environment(environment_id, &auth)
            .await?;

        Ok(Json(Page {
            values: result.into_iter().map(|ss| ss.into()).collect(),
        }))
    }

    /// Get security scheme
    #[oai(
        path = "/security-schemes/:security_scheme_id",
        method = "get",
        operation_id = "get_security_scheme"
    )]
    pub async fn get_security_scheme(
        &self,
        security_scheme_id: Path<SecuritySchemeId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<SecuritySchemeDto>> {
        let record = recorded_http_api_request!(
            "get_security_scheme",
            security_scheme_id = security_scheme_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_security_scheme_internal(security_scheme_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_security_scheme_internal(
        &self,
        security_scheme_id: SecuritySchemeId,
        auth: AuthCtx,
    ) -> ApiResult<Json<SecuritySchemeDto>> {
        let security_scheme = self
            .security_scheme_service
            .get(security_scheme_id, &auth)
            .await?;
        Ok(Json(security_scheme.into()))
    }

    /// Update security scheme
    #[oai(
        path = "/security-schemes/:security_scheme_id",
        method = "patch",
        operation_id = "update_security_scheme"
    )]
    pub async fn update_security_scheme(
        &self,
        security_scheme_id: Path<SecuritySchemeId>,
        data: Json<SecuritySchemeUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<SecuritySchemeDto>> {
        let record = recorded_http_api_request!(
            "update_security_scheme",
            security_scheme_id = security_scheme_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_security_scheme_internal(security_scheme_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_security_scheme_internal(
        &self,
        security_scheme_id: SecuritySchemeId,
        data: SecuritySchemeUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<SecuritySchemeDto>> {
        let security_scheme = self
            .security_scheme_service
            .update(security_scheme_id, data, &auth)
            .await?;
        Ok(Json(security_scheme.into()))
    }

    /// Delete security scheme
    #[oai(
        path = "/security-schemes/:security_scheme_id",
        method = "delete",
        operation_id = "delete_security_scheme"
    )]
    pub async fn delete_security_scheme(
        &self,
        security_scheme_id: Path<SecuritySchemeId>,
        current_revision: Query<SecuritySchemeRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<SecuritySchemeDto>> {
        let record = recorded_http_api_request!(
            "delete_security_scheme",
            security_scheme_id = security_scheme_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_security_scheme_internal(security_scheme_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_security_scheme_internal(
        &self,
        security_scheme_id: SecuritySchemeId,
        current_revision: SecuritySchemeRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<SecuritySchemeDto>> {
        let security_scheme = self
            .security_scheme_service
            .delete(security_scheme_id, current_revision, &auth)
            .await?;
        Ok(Json(security_scheme.into()))
    }
}
