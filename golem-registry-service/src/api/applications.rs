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
use crate::services::application::ApplicationService;
use crate::services::auth::AuthService;
use golem_common::model::Page;
use golem_common::model::account::AccountId;
use golem_common::model::application::{
    Application, ApplicationCreation, ApplicationId, ApplicationName, ApplicationRevision,
    ApplicationUpdate,
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

pub struct ApplicationsApi {
    application_service: Arc<ApplicationService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Application
)]
impl ApplicationsApi {
    pub fn new(
        application_service: Arc<ApplicationService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            application_service,
            auth_service,
        }
    }

    /// Create an application in the account
    #[oai(
        path = "/accounts/:account_id/apps",
        method = "post",
        operation_id = "create_application",
        tag = ApiTags::Account,
    )]
    pub async fn create_application(
        &self,
        account_id: Path<AccountId>,
        data: Json<ApplicationCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!(
            "create_application",
            account_id = account_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_application_internal(account_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_application_internal(
        &self,
        account_id: AccountId,
        data: ApplicationCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<Application>> {
        let result = self
            .application_service
            .create(account_id, data, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Get application in the account by name
    #[oai(
        path = "/accounts/:account_id/apps/:application_name",
        method = "get",
        operation_id = "get_account_application",
        tag = ApiTags::Account,
    )]
    pub async fn get_account_application(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<ApplicationName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!(
            "get_account_application",
            account_id = account_id.0.to_string(),
            application_name = application_name.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_account_application_internal(account_id.0, application_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_account_application_internal(
        &self,
        account_id: AccountId,
        application_name: ApplicationName,
        auth: AuthCtx,
    ) -> ApiResult<Json<Application>> {
        let application = self
            .application_service
            .get_in_account(account_id, &application_name, &auth)
            .await?;
        Ok(Json(application))
    }

    /// Get all applications in the account
    #[oai(
        path = "/accounts/:account_id/apps",
        method = "get",
        operation_id = "list_account_applications",
        tag = ApiTags::Account,
    )]
    pub async fn list_account_applications(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Application>>> {
        let record = recorded_http_api_request!(
            "list_account_applications",
            account_id = account_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_account_applications_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_account_applications_internal(
        &self,
        account_id: AccountId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<Application>>> {
        let applications = self
            .application_service
            .list_in_account(account_id, &auth)
            .await?;
        Ok(Json(Page {
            values: applications,
        }))
    }

    /// Get application by id.
    #[oai(
        path = "/apps/:application_id",
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
        let application = self.application_service.get(application_id, &auth).await?;
        Ok(Json(application))
    }

    /// Update application by id.
    #[oai(
        path = "/apps/:application_id",
        method = "patch",
        operation_id = "update_application"
    )]
    pub async fn update_application(
        &self,
        application_id: Path<ApplicationId>,
        payload: Json<ApplicationUpdate>,
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
        payload: ApplicationUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<Application>> {
        let application = self
            .application_service
            .update(application_id, payload, &auth)
            .await?;
        Ok(Json(application))
    }

    /// Update application by id.
    #[oai(
        path = "/apps/:application_id",
        method = "delete",
        operation_id = "delete_application"
    )]
    pub async fn delete_application(
        &self,
        application_id: Path<ApplicationId>,
        current_revision: Query<ApplicationRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<NoContentResponse> {
        let record = recorded_http_api_request!(
            "delete_application",
            application_id = application_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_application_internal(application_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_application_internal(
        &self,
        application_id: ApplicationId,
        current_revision: ApplicationRevision,
        auth: AuthCtx,
    ) -> ApiResult<NoContentResponse> {
        self.application_service
            .delete(application_id, current_revision, &auth)
            .await?;

        Ok(NoContentResponse::NoContent)
    }
}
