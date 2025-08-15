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
use golem_common::model::account::AccountId;
use golem_common::model::application::{Application, NewApplicationData};
use golem_common::model::auth::AuthCtx;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use tracing::Instrument;
use crate::services::application::ApplicationService;
use std::sync::Arc;
use uuid::Uuid;

pub struct AccountApplicationsApi {
    application_service: Arc<ApplicationService>
}

#[OpenApi(
    prefix_path = "/v1/accounts",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Account,
    tag = ApiTags::Application
)]
impl AccountApplicationsApi {
    pub fn new(
        application_service: Arc<ApplicationService>
    ) -> Self {
        Self {
            application_service
        }
    }

    /// Get all applications in the account
    #[oai(
        path = "/:account_id/apps",
        method = "get",
        operation_id = "get_applications_in_account"
    )]
    pub async fn get_applications_in_account(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Application>>> {
        let record = recorded_http_api_request!(
            "get_applications_in_account",
            account_id = account_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_applications_in_account_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_applications_in_account_internal(
        &self,
        _account_id: AccountId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<Application>>> {
        todo!()
    }

    /// Get application in the account by name
    #[oai(
        path = "/:account_id/apps/:application_name",
        method = "get",
        operation_id = "get_account_application"
    )]
    pub async fn get_application(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!(
            "get_account_application",
            account_id = account_id.0.to_string(),
            application_name = application_name.0
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_application_internal(account_id.0, application_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_application_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Application>> {
        todo!()
    }

    /// Create an application in the account
    #[oai(
        path = "/:account_id/apps",
        method = "post",
        operation_id = "create_application"
    )]
    pub async fn create_application(
        &self,
        account_id: Path<AccountId>,
        data: Json<NewApplicationData>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!(
            "create_application",
            account_id = account_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .create_application_internal(account_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_application_internal(
        &self,
        account_id: AccountId,
        data: NewApplicationData,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Application>> {
        // TODO
        let actor = AccountId(Uuid::nil());
        let result = self.application_service.create(account_id, data, actor).await?;

        Ok(Json(result))
    }
}
