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

use golem_common::model::auth::AuthCtx;
use golem_common::model::error::ErrorBody;
use golem_common::model::{AccountId, Empty, PluginId};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;
use std::sync::Arc;
use tracing::Instrument;
use super::ApiResult;
use super::model::{Application, ApplicationData, Environment, EnvironmentData, Page};
use super::model::plugins::*;

pub struct AccountApplicationsApi { }

#[OpenApi(prefix_path = "/v1/accounts/:account_id/apps", tag = ApiTags::Plugin)]
impl AccountApplicationsApi {

    /// Get all applications in the account.
    #[oai(
        path = "/",
        method = "get",
        operation_id = "list_applications"
    )]
    pub async fn list_applications(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Application>>> {
        let record = recorded_http_api_request!("list_plugins",);

        let response = self
            .list_applications_internal(account_id.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_applications_internal(
        &self,
        _account_id: AccountId,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Application>>> {
        todo!()
    }

    /// Get application by name
    #[oai(
        path = "/:application_name",
        method = "get",
        operation_id = "get_application_by_name"
    )]
    pub async fn get_application_by_name(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!("get_application_by_name",);

        let response = self
            .get_application_by_name_internal(account_id.0, application_name.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_application_by_name_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        todo!()
    }

    /// Update or create an application by name
    #[oai(
        path = "/:application_name",
        method = "put",
        operation_id = "put_application_by_name"
    )]
    pub async fn put_application_by_name(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        data: Json<ApplicationData>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!("put_application_by_name",);

        let response = self
            .put_application_by_name_internal(account_id.0, application_name.0, data.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn put_application_by_name_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _data: ApplicationData,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        todo!()
    }

    /// List all application environments
    #[oai(
        path = "/:application_name/envs",
        method = "get",
        operation_id = "list_application_environments"
    )]
    pub async fn list_application_environments(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Environment>>> {
        let record = recorded_http_api_request!("list_application_environments",);

        let response = self
            .list_application_environments_internal(account_id.0, application_name.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_application_environments_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Environment>>> {
        todo!()
    }

    /// Get application environment by name
    #[oai(
        path = "/:application_name/envs/:environment_name",
        method = "get",
        operation_id = "get_application_environment_by_name"
    )]
    pub async fn get_application_environment_by_name(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        environment_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!("get_application_environment_by_name",);

        let response = self
            .get_application_environment_by_name_internal(account_id.0, application_name.0, environment_name.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_application_environment_by_name_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _environment_name: String,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        todo!()
    }

    /// Create or update application environment by name
    #[oai(
        path = "/:application_name/envs/:environment_name",
        method = "put",
        operation_id = "put_application_environment_by_name"
    )]
    pub async fn put_application_environment_by_name(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        environment_name: Path<String>,
        data: Json<EnvironmentData>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!("get_application_environment_by_name",);

        let response = self
            .put_application_environment_by_name_internal(account_id.0, application_name.0, environment_name.0, data.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn put_application_environment_by_name_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _environment_name: String,
        _data: EnvironmentData,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        todo!()
    }

}
