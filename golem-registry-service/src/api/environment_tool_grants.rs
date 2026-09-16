// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use crate::services::environment_tool_grant::EnvironmentToolGrantService;
use golem_common::model::Page;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_tool_grant::{
    EnvironmentToolGrantCreation, EnvironmentToolGrantDeletion, EnvironmentToolGrantId,
    EnvironmentToolGrantWithDetails, EnvironmentToolValidation, EnvironmentToolValidationResult,
};
use golem_common::model::poem::NoContentResponse;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct EnvironmentToolGrantsApi {
    environment_tool_grant_service: Arc<EnvironmentToolGrantService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::EnvironmentToolGrants
)]
impl EnvironmentToolGrantsApi {
    pub fn new(
        environment_tool_grant_service: Arc<EnvironmentToolGrantService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            environment_tool_grant_service,
            auth_service,
        }
    }

    /// Grant an exact published tool release to an environment
    #[oai(
        path = "/envs/:environment_id/tools",
        method = "post",
        operation_id = "create_environment_tool_grant",
        tag = ApiTags::Environment
    )]
    async fn create_environment_tool_grant(
        &self,
        environment_id: Path<EnvironmentId>,
        creation: Json<EnvironmentToolGrantCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentToolGrantWithDetails>> {
        let record = recorded_http_api_request!(
            "create_environment_tool_grant",
            environment_id = environment_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let result = async {
            Ok(Json(
                self.environment_tool_grant_service
                    .create(environment_id.0, creation.0, &auth)
                    .await?,
            ))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    /// Validate environment tool grants and publications without changing them
    #[oai(
        path = "/envs/:environment_id/tools/validate",
        method = "post",
        operation_id = "validate_environment_tools",
        tag = ApiTags::Environment
    )]
    async fn validate_environment_tools(
        &self,
        environment_id: Path<EnvironmentId>,
        validation: Json<EnvironmentToolValidation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentToolValidationResult>> {
        let record = recorded_http_api_request!(
            "validate_environment_tools",
            environment_id = environment_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let result = async {
            Ok(Json(
                self.environment_tool_grant_service
                    .validate_tools(environment_id.0, validation.0, &auth)
                    .await?,
            ))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    /// List active tool grants in an environment
    #[oai(
        path = "/envs/:environment_id/tools",
        method = "get",
        operation_id = "list_environment_tool_grants",
        tag = ApiTags::Environment
    )]
    async fn list_environment_tool_grants(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<EnvironmentToolGrantWithDetails>>> {
        let record = recorded_http_api_request!(
            "list_environment_tool_grants",
            environment_id = environment_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let result = async {
            Ok(Json(Page {
                values: self
                    .environment_tool_grant_service
                    .list_in_environment(environment_id.0, &auth)
                    .await?,
            }))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    /// Get an active environment tool grant
    #[oai(
        path = "/environment-tools/:grant_id",
        method = "get",
        operation_id = "get_environment_tool_grant",
        tag = ApiTags::EnvironmentToolGrants
    )]
    async fn get_environment_tool_grant(
        &self,
        grant_id: Path<EnvironmentToolGrantId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentToolGrantWithDetails>> {
        let record = recorded_http_api_request!(
            "get_environment_tool_grant",
            grant_id = grant_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let result = async {
            Ok(Json(
                self.environment_tool_grant_service
                    .get(grant_id.0, &auth)
                    .await?,
            ))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    /// Delete an environment tool grant
    #[oai(
        path = "/environment-tools/:grant_id",
        method = "delete",
        operation_id = "delete_environment_tool_grant",
        tag = ApiTags::EnvironmentToolGrants
    )]
    async fn delete_environment_tool_grant(
        &self,
        grant_id: Path<EnvironmentToolGrantId>,
        deletion: Json<EnvironmentToolGrantDeletion>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        let record = recorded_http_api_request!(
            "delete_environment_tool_grant",
            grant_id = grant_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let result = async {
            self.environment_tool_grant_service
                .delete(grant_id.0, deletion.0.automatic, &auth)
                .await?;
            Ok(NoContentResponse::NoContent)
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    /// Restore a deleted environment tool grant
    #[oai(
        path = "/environment-tools/:grant_id/restore",
        method = "post",
        operation_id = "restore_environment_tool_grant",
        tag = ApiTags::EnvironmentToolGrants
    )]
    async fn restore_environment_tool_grant(
        &self,
        grant_id: Path<EnvironmentToolGrantId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentToolGrantWithDetails>> {
        let record = recorded_http_api_request!(
            "restore_environment_tool_grant",
            grant_id = grant_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let result = async {
            Ok(Json(
                self.environment_tool_grant_service
                    .restore(grant_id.0, &auth)
                    .await?,
            ))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }
}
