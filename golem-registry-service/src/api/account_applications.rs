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
use golem_common_next::api::Page;
use golem_common_next::model::AccountId;
use golem_common_next::model::environment::*;
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use tracing::Instrument;
use golem_common_next::model::auth::AuthCtx;

pub struct AccountApplicationsApi {}

#[OpenApi(prefix_path = "/v1/accounts/:account_id/apps", tag = ApiTags::Plugin)]
impl AccountApplicationsApi {
    /// Get all applications in the account.
    #[oai(path = "/", method = "get", operation_id = "list_applications")]
    pub async fn list_applications(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Application>>> {
        let record = recorded_http_api_request!(
            "list_plugins",
            account_id = account_id.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .list_applications_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_applications_internal(
        &self,
        _account_id: AccountId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<Application>>> {
        todo!()
    }

    /// Get application by name
    #[oai(
        path = "/:application_name",
        method = "get",
        operation_id = "get_application"
    )]
    pub async fn get_application(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!("get_application",);

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

    /// Create an application by name
    #[oai(
        path = "/:application_name",
        method = "post",
        operation_id = "create_application"
    )]
    pub async fn create_application(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        data: Json<CreateApplicationRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!(
            "create_application",
            account_id = account_id.0.to_string(),
            application_name = application_name.0
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .create_application_internal(account_id.0, application_name.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_application_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _data: CreateApplicationRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Application>> {
        todo!()
    }

    /// Update an application by name
    #[oai(
        path = "/:application_name",
        method = "patch",
        operation_id = "update_application"
    )]
    pub async fn update_application(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        data: Json<UpdateApplicationRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Application>> {
        let record = recorded_http_api_request!(
            "update_application",
            account_id = account_id.0.to_string(),
            application_name = application_name.0
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .update_application_internal(account_id.0, application_name.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_application_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _data: UpdateApplicationRequest,
        _auth: AuthCtx,
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
        let record = recorded_http_api_request!(
            "list_application_environments",
            account_id = account_id.0.to_string(),
            application_name = application_name.0
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .list_application_environments_internal(account_id.0, application_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_application_environments_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<Environment>>> {
        todo!()
    }

    /// Get application environment by name
    #[oai(
        path = "/:application_name/envs/:environment_name",
        method = "get",
        operation_id = "get_application_environment"
    )]
    pub async fn get_application_environment(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        environment_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "get_application_environment",
            account_id = account_id.0.to_string(),
            application_name = application_name.0,
            environment_name = environment_name.0
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_application_environment_internal(
                account_id.0,
                application_name.0,
                environment_name.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_application_environment_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _environment_name: String,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        todo!()
    }

    /// Create an application environment by name
    #[oai(
        path = "/:application_name/envs/:environment_name",
        method = "post",
        operation_id = "create_application_environment"
    )]
    pub async fn create_application_environment(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        environment_name: Path<String>,
        data: Json<CreateEnvironmentRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "create_application_environment",
            account_id = account_id.0.to_string(),
            application_name = application_name.0,
            environment_name = environment_name.0
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .create_application_environment_internal(
                account_id.0,
                application_name.0,
                environment_name.0,
                data.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_application_environment_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _environment_name: String,
        _data: CreateEnvironmentRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        todo!()
    }

    /// Update an application environment by name
    #[oai(
        path = "/:application_name/envs/:environment_name",
        method = "patch",
        operation_id = "update_application_environment"
    )]
    pub async fn update_application_environment(
        &self,
        account_id: Path<AccountId>,
        application_name: Path<String>,
        environment_name: Path<String>,
        data: Json<UpdateEnvironmentRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Environment>> {
        let record = recorded_http_api_request!(
            "update_application_environment",
            account_id = account_id.0.to_string(),
            application_name = application_name.0,
            environment_name = environment_name.0
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .update_application_environment_internal(
                account_id.0,
                application_name.0,
                environment_name.0,
                data.0,
                auth,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_application_environment_internal(
        &self,
        _account_id: AccountId,
        _application_name: String,
        _environment_name: String,
        _data: UpdateEnvironmentRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Environment>> {
        todo!()
    }
}
