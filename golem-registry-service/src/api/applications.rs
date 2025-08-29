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
use crate::model::auth::AuthCtx;
use crate::services::application::ApplicationService;
use crate::services::auth::AuthService;
use crate::services::environment::EnvironmentService;
use golem_common::api::Page;
use golem_common::model::application::{Application, ApplicationId, UpdatedApplicationData};
use golem_common::model::environment::*;
use golem_common::model::poem::NoContentResponse;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct ApplicationsApi {
    application_service: Arc<ApplicationService>,
    environment_service: Arc<EnvironmentService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/apps",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Application
)]
impl ApplicationsApi {
    pub fn new(
        application_service: Arc<ApplicationService>,
        environment_service: Arc<EnvironmentService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            application_service,
            environment_service,
            auth_service,
        }
    }

    /// Get application by id.
    #[oai(
        path = "/:application_id",
        method = "get",
        operation_id = "get_application"
    )]
    pub async fn get_application(
        &self,
        application_id: Path<ApplicationId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!(
            "get_application",
            application_id = application_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_application_internal(application_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_application_internal(
        &self,
        application_id: ApplicationId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Application>> {
        let application = self.application_service.get(&application_id, &auth).await?;
        Ok(Json(application))
    }

    /// Update application by id.
    #[oai(
        path = "/:application_id",
        method = "patch",
        operation_id = "update_application"
    )]
    pub async fn update_application(
        &self,
        application_id: Path<ApplicationId>,
        payload: Json<UpdatedApplicationData>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!(
            "update_application",
            application_id = application_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_application_internal(application_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_application_internal(
        &self,
        application_id: ApplicationId,
        payload: UpdatedApplicationData,
        auth: AuthCtx,
    ) -> ApiResult<Json<Application>> {
        let application = self
            .application_service
            .update(&application_id, payload, &auth)
            .await?;
        Ok(Json(application))
    }

    /// Update application by id.
    #[oai(
        path = "/:application_id",
        method = "delete",
        operation_id = "delete_application"
    )]
    pub async fn delete_application(
        &self,
        application_id: Path<ApplicationId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        let record = recorded_http_api_request!(
            "delete_application",
            application_id = application_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_application_internal(application_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_application_internal(
        &self,
        application_id: ApplicationId,
        auth: AuthCtx,
    ) -> ApiResult<NoContentResponse> {
        self.application_service
            .delete(&application_id, &auth)
            .await?;

        Ok(NoContentResponse::NoContent)
    }

    /// List all application environments
    #[oai(
        path = "/:application_id/envs",
        method = "get",
        operation_id = "list_application_environments",
        tag = ApiTags::Environment
    )]
    pub async fn list_application_environments(
        &self,
        application_id: Path<ApplicationId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Environment>>> {
        let record = recorded_http_api_request!(
            "list_application_environments",
            application_id = application_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_application_environments_internal(application_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_application_environments_internal(
        &self,
        application_id: ApplicationId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<Environment>>> {
        let result = self
            .environment_service
            .list_in_application(&application_id, &auth)
            .await?;
        Ok(Json(Page {
            values: result.into_iter().map(|e| e.value).collect(),
        }))
    }

    /// Create an application environment
    #[oai(
        path = "/:application_id/envs",
        method = "post",
        operation_id = "create_environment",
        tag = ApiTags::Environment
    )]
    pub async fn create_environment(
        &self,
        application_id: Path<ApplicationId>,
        data: Json<NewEnvironmentData>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "create_environment",
            application_id = application_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_environment_internal(application_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_environment_internal(
        &self,
        application_id: ApplicationId,
        data: NewEnvironmentData,
        auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        let result = self
            .environment_service
            .create(application_id, data, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Get application environment by name
    #[oai(
        path = "/:application_id/envs/:environment_name",
        method = "get",
        operation_id = "get_application_environment",
        tag = ApiTags::Environment
    )]
    pub async fn get_application_environment(
        &self,
        application_id: Path<ApplicationId>,
        environment_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "get_application_environment",
            application_id = application_id.0.to_string(),
            environment_name = environment_name.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_application_environment_internal(application_id.0, environment_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_application_environment_internal(
        &self,
        application_id: ApplicationId,
        environment_name: String,
        auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        let environment = self
            .environment_service
            .get_in_application(&application_id, &EnvironmentName(environment_name), &auth)
            .await?;
        Ok(Json(environment.value))
    }
}
