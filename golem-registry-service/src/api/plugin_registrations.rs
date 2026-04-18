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
use crate::services::plugin_registration::PluginRegistrationService;
use golem_common::model::Page;
use golem_common::model::account::AccountId;
use golem_common::model::plugin_registration::{
    PluginRegistrationCreation, PluginRegistrationDto, PluginRegistrationId,
};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct PluginRegistrationsApi {
    plugin_registration_service: Arc<PluginRegistrationService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Plugin
)]
impl PluginRegistrationsApi {
    pub fn new(
        plugin_registration_service: Arc<PluginRegistrationService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            plugin_registration_service,
            auth_service,
        }
    }

    /// Register a new plugin
    #[oai(
        path = "/accounts/:account_id/plugins",
        method = "post",
        operation_id = "create_plugin",
        tag = ApiTags::Account
    )]
    async fn create_plugin(
        &self,
        account_id: Path<AccountId>,
        payload: Json<PluginRegistrationCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<PluginRegistrationDto>> {
        let record = recorded_http_api_request!(
            "create_plugin",
            plugin_name = payload.0.name,
            plugin_version = payload.0.version
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_plugin_internal(account_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_plugin_internal(
        &self,
        account_id: AccountId,
        metadata: PluginRegistrationCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<PluginRegistrationDto>> {
        let plugin_registration = self
            .plugin_registration_service
            .register_plugin(account_id, metadata, &auth)
            .await?;
        Ok(Json(plugin_registration.into()))
    }

    /// List all plugins registered in account
    #[oai(
        path = "/accounts/:account_id/plugins",
        method = "get",
        operation_id = "list_account_plugins",
        tag = ApiTags::Account
    )]
    async fn list_account_plugins(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<PluginRegistrationDto>>> {
        let record = recorded_http_api_request!(
            "list_account_plugins",
            account_id = account_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_account_plugins_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_account_plugins_internal(
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

    /// Get a plugin by id
    #[oai(
        path = "/plugins/:plugin_id",
        method = "get",
        operation_id = "get_plugin_by_id"
    )]
    async fn get_plugin_by_id(
        &self,
        plugin_id: Path<PluginRegistrationId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<PluginRegistrationDto>> {
        let record =
            recorded_http_api_request!("get_plugin_by_id", plugin_id = plugin_id.0.to_string(),);

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_plugin_by_id_internal(plugin_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_plugin_by_id_internal(
        &self,
        plugin_id: PluginRegistrationId,
        auth: AuthCtx,
    ) -> ApiResult<Json<PluginRegistrationDto>> {
        let plugin_registration = self
            .plugin_registration_service
            .get_plugin(plugin_id, false, &auth)
            .await?;
        Ok(Json(plugin_registration.into()))
    }

    /// Delete a plugin
    #[oai(
        path = "/plugins/:plugin_id",
        method = "delete",
        operation_id = "delete_plugin"
    )]
    async fn delete_plugin(
        &self,
        plugin_id: Path<PluginRegistrationId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<PluginRegistrationDto>> {
        let record =
            recorded_http_api_request!("delete_plugin", plugin_id = plugin_id.0.to_string(),);

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_plugin_internal(plugin_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_plugin_internal(
        &self,
        plugin_id: PluginRegistrationId,
        auth: AuthCtx,
    ) -> ApiResult<Json<PluginRegistrationDto>> {
        let plugin_registration = self
            .plugin_registration_service
            .unregister_plugin(plugin_id, &auth)
            .await?;
        Ok(Json(plugin_registration.into()))
    }
}
