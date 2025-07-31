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
use golem_common::model::{AccountId, Empty};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;
use std::sync::Arc;
use tracing::Instrument;
use super::ApiResult;
use super::model::Page;
use super::model::plugins::*;

pub struct PluginApi { }

#[OpenApi(prefix_path = "/v1/plugins", tag = ApiTags::Plugin)]
impl PluginApi {

    /// Lists all the registered plugins (including all versions of each).
    #[oai(path = "/", method = "get", operation_id = "list_plugins")]
    pub async fn list_plugins(
        &self,
        scope: Query<PluginScope>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<PluginDefinition>>> {
        let record = recorded_http_api_request!("list_plugins",);

        let response = self
            .list_plugins_internal(scope.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_plugins_internal(
        &self,
        _scope: PluginScope,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<PluginDefinition>>> {
        todo!()
    }

    /// Gets a registered plugin by its name and version
    #[oai(
        path = "/:account_id/:name/:version",
        method = "get",
        operation_id = "get_plugin"
    )]
    pub async fn get_plugin(
        &self,
        account_id: Path<AccountId>,
        name: Path<String>,
        version: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<PluginDefinition>> {
        let record = recorded_http_api_request!(
            "get_plugin",
            plugin_name = name.0,
            plugin_version = version.0
        );

        let response = self
            .get_plugin_internal(account_id.0, name.0, version.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_plugin_internal(
        &self,
        _account_id: AccountId,
        _name: String,
        _version: String,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<PluginDefinition>> {
        todo!()
    }

    /// Deletes a registered plugin by its name and version
    #[oai(
        path = "/:account_id/:name/:version",
        method = "delete",
        operation_id = "delete_plugin"
    )]
    pub async fn delete_plugin(
        &self,
        account_id: Path<AccountId>,
        name: Path<String>,
        version: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        let record = recorded_http_api_request!(
            "delete_plugin",
            plugin_name = name.0,
            plugin_version = version.0
        );

        let response = self
            .delete_plugin_internal(account_id.0, name.0, version.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_plugin_internal(
        &self,
        _account_id: AccountId,
        _name: String,
        _version: String,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        todo!()
    }

    /// Registers a new plugin
    #[oai(path = "/", method = "post", operation_id = "create_plugin")]
    pub async fn create_plugin(
        &self,
        plugin: Json<CreatePluginRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        let record = recorded_http_api_request!(
            "create_plugin",
            plugin_name = plugin.name,
            plugin_version = plugin.version
        );

        let response = self
            .create_plugin_internal(plugin.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_plugin_internal(
        &self,
        _plugin: CreatePluginRequest,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        todo!()
    }

    /// Registers a new library plugin
    #[oai(
        path = "/library-plugins/",
        method = "post",
        operation_id = "create_library_plugin"
    )]
    pub async fn create_library_plugin(
        &self,
        plugin: CreateLibraryPluginRequest,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        let record = recorded_http_api_request!(
            "create_library_plugin",
            plugin_name = plugin.name,
            plugin_version = plugin.version
        );

        let response = self
            .create_library_plugin_internal(plugin, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_library_plugin_internal(
        &self,
        _plugin: CreateLibraryPluginRequest,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        todo!()
    }


    /// Registers a new app plugin
    #[oai(
        path = "/app-plugins/",
        method = "post",
        operation_id = "create_app_plugin"
    )]
    pub async fn create_app_plugin(
        &self,
        plugin: CreateAppPluginRequest,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        let record = recorded_http_api_request!(
            "create_app_plugin",
            plugin_name = plugin.name,
            plugin_version = plugin.version
        );

        let response = self
            .create_app_plugin_internal(plugin, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_app_plugin_internal(
        &self,
        _plugin: CreateAppPluginRequest,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        todo!()
    }
}
