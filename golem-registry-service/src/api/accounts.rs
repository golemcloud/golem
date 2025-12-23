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
use crate::services::account::AccountService;
use crate::services::auth::AuthService;
use crate::services::plan::PlanService;
use crate::services::plugin_registration::PluginRegistrationService;
use golem_common::model::Empty;
use golem_common::model::Page;
use golem_common::model::account::{
    Account, AccountCreation, AccountId, AccountRevision, AccountSetPlan, AccountSetRoles,
    AccountUpdate,
};
use golem_common::model::plan::Plan;
use golem_common::model::plugin_registration::{PluginRegistrationCreation, PluginRegistrationDto};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use golem_service_base::poem::TempFileUpload;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::types::multipart::JsonField;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct AccountsApi {
    account_service: Arc<AccountService>,
    plan_service: Arc<PlanService>,
    auth_service: Arc<AuthService>,
    plugin_registration_service: Arc<PluginRegistrationService>,
}

#[OpenApi(
    prefix_path = "/v1/accounts",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Account
)]
impl AccountsApi {
    pub fn new(
        account_service: Arc<AccountService>,
        plan_service: Arc<PlanService>,
        auth_service: Arc<AuthService>,
        plugin_registration_service: Arc<PluginRegistrationService>,
    ) -> Self {
        Self {
            account_service,
            plan_service,
            auth_service,
            plugin_registration_service,
        }
    }

    /// Create account
    ///
    /// Create a new account. The response is the created account data.
    #[oai(path = "/", method = "post", operation_id = "create_account")]
    async fn create_account(
        &self,
        data: Json<AccountCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record = recorded_http_api_request!("create_account", account_name = data.name.clone());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_account_internal(data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_account_internal(
        &self,
        data: AccountCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        let result = self.account_service.create(data, &auth).await?;
        Ok(Json(result))
    }

    /// Get account
    ///
    /// Retrieve an account for a given Account ID
    #[oai(path = "/:account_id", method = "get", operation_id = "get_account")]
    async fn get_account(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record =
            recorded_http_api_request!("get_account", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_account_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_account_internal(
        &self,
        account_id: AccountId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        let result = self.account_service.get(account_id, &auth).await?;
        Ok(Json(result))
    }

    /// Get account's plan
    #[oai(
        path = "/:account_id/plan",
        method = "get",
        operation_id = "get_account_plan"
    )]
    async fn get_account_plan(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Plan>> {
        let record =
            recorded_http_api_request!("get_account_plan", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_account_plan_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_account_plan_internal(
        &self,
        account_id: AccountId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Plan>> {
        let account = self.account_service.get(account_id, &auth).await?;
        let plan = self.plan_service.get(&account.plan_id, &auth).await?;

        Ok(Json(plan))
    }

    /// Update account
    ///
    /// Allows the user to change the account details such as name and email.
    ///
    /// Changing the planId is not allowed and the request will be rejected.
    /// The response is the updated account data.
    #[oai(path = "/:account_id", method = "put", operation_id = "update_account")]
    async fn put_account(
        &self,
        account_id: Path<AccountId>,
        data: Json<AccountUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record =
            recorded_http_api_request!("update_account", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .put_account_internal(account_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn put_account_internal(
        &self,
        account_id: AccountId,
        data: AccountUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        let result = self.account_service.update(account_id, data, &auth).await?;
        Ok(Json(result))
    }

    /// Update roles of an accout
    #[oai(
        path = "/:account_id/roles",
        method = "put",
        operation_id = "set_account_roles"
    )]
    async fn set_account_roles(
        &self,
        account_id: Path<AccountId>,
        payload: Json<AccountSetRoles>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record =
            recorded_http_api_request!("set_account_roles", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .set_account_roles_internal(account_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn set_account_roles_internal(
        &self,
        account_id: AccountId,
        payload: AccountSetRoles,
        auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        let result = self
            .account_service
            .set_roles(account_id, payload, &auth)
            .await?;
        Ok(Json(result))
    }

    /// Update plan of an accout
    #[oai(
        path = "/:account_id/plan",
        method = "put",
        operation_id = "set_account_plan"
    )]
    async fn set_account_plan(
        &self,
        account_id: Path<AccountId>,
        payload: Json<AccountSetPlan>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record =
            recorded_http_api_request!("set_account_plan", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .set_account_plan_internal(account_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn set_account_plan_internal(
        &self,
        account_id: AccountId,
        payload: AccountSetPlan,
        auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        let result = self
            .account_service
            .set_plan(account_id, payload, &auth)
            .await?;
        Ok(Json(result))
    }

    /// Delete account
    ///
    /// Delete an account.
    #[oai(
        path = "/:account_id",
        method = "delete",
        operation_id = "delete_account"
    )]
    async fn delete_account(
        &self,
        account_id: Path<AccountId>,
        current_revision: Query<AccountRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        let record =
            recorded_http_api_request!("delete_account", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_account_internal(account_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_account_internal(
        &self,
        account_id: AccountId,
        current_revision: AccountRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<Empty>> {
        self.account_service
            .delete(account_id, current_revision, &auth)
            .await?;
        Ok(Json(Empty {}))
    }

    /// Register a new plugin
    #[oai(
        path = "/:account_id/plugins",
        method = "post",
        operation_id = "create_plugin",
        tag = ApiTags::Plugin
    )]
    async fn create_plugin(
        &self,
        account_id: Path<AccountId>,
        payload: CreatePluginRequest,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<PluginRegistrationDto>> {
        let record = recorded_http_api_request!(
            "create_plugin",
            plugin_name = payload.metadata.0.name,
            plugin_version = payload.metadata.0.version
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_plugin_internal(account_id.0, payload.metadata.0, payload.plugin_wasm, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_plugin_internal(
        &self,
        account_id: AccountId,
        metadata: PluginRegistrationCreation,
        plugin_wasm: Option<TempFileUpload>,
        auth: AuthCtx,
    ) -> ApiResult<Json<PluginRegistrationDto>> {
        let plugin_registration = self
            .plugin_registration_service
            .register_plugin(
                account_id,
                metadata,
                plugin_wasm.map(|pw| pw.into_file()),
                &auth,
            )
            .await?;
        Ok(Json(plugin_registration.into()))
    }

    /// Get all plugins registered in account
    #[oai(
        path = "/:account_id/plugins",
        method = "get",
        operation_id = "get_account_plugins",
        tag = ApiTags::Plugin
    )]
    async fn get_account_plugins(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<PluginRegistrationDto>>> {
        let record = recorded_http_api_request!(
            "get_account_plugins",
            account_id = account_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_account_plugins_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_account_plugins_internal(
        &self,
        account_id: AccountId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<PluginRegistrationDto>>> {
        let plugin_registrations = self
            .plugin_registration_service
            .list_plugins_in_account(account_id, &auth)
            .await?;
        Ok(Json(Page {
            values: plugin_registrations
                .into_iter()
                .map(|pr| pr.into())
                .collect(),
        }))
    }
}

#[derive(Debug, poem_openapi::Multipart)]
#[oai(rename_all = "camelCase")]
struct CreatePluginRequest {
    metadata: JsonField<PluginRegistrationCreation>,
    plugin_wasm: Option<TempFileUpload>,
}
