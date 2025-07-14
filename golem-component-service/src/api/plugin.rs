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

use super::ComponentError;
use crate::api::dto;
use crate::api::Result;
use crate::authed::plugin::AuthedPluginService;
use golem_common::model::auth::AuthCtx;
use golem_common::model::error::ErrorBody;
use golem_common::model::plugin::PluginDefinition;
use golem_common::model::plugin::PluginScope;
use golem_common::model::{AccountId, Empty};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;
use std::sync::Arc;
use tracing::Instrument;

pub struct PluginApi {
    plugin_service: Arc<AuthedPluginService>,
}

#[OpenApi(prefix_path = "/v1/plugins", tag = ApiTags::Plugin)]
impl PluginApi {
    pub fn new(plugin_service: Arc<AuthedPluginService>) -> Self {
        Self { plugin_service }
    }

    /// Lists all the registered plugins (including all versions of each).
    #[oai(path = "/", method = "get", operation_id = "list_plugins")]
    pub async fn list_plugins(
        &self,
        scope: Query<PluginScope>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<PluginDefinition>>> {
        let record = recorded_http_api_request!("list_plugins",);
        let auth = AuthCtx::new(token.secret());

        let response = self
            .plugin_service
            .list_plugins_for_scope(&auth, &scope)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|response| Json(response.into_iter().collect()));

        record.result(response)
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
    ) -> Result<Json<PluginDefinition>> {
        let record = recorded_http_api_request!(
            "get_plugin",
            plugin_name = name.0,
            plugin_version = version.0
        );
        let auth = AuthCtx::new(token.secret());

        let response = self
            .plugin_service
            .get(&auth, account_id.0, &name, &version)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .and_then(|response| match response {
                Some(response) => Ok(Json(response)),
                None => Err(ComponentError::NotFound(Json(ErrorBody {
                    error: "Plugin not found".to_string(),
                }))),
            });

        record.result(response)
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
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "delete_plugin",
            plugin_name = name.0,
            plugin_version = version.0
        );
        let auth = AuthCtx::new(token.secret());

        let response = self
            .plugin_service
            .delete(&auth, account_id.0, &name, &version)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }

    /// Registers a new plugin
    #[oai(path = "/", method = "post", operation_id = "create_plugin")]
    pub async fn create_plugin(
        &self,
        plugin: Json<dto::PluginDefinitionCreation>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "create_plugin",
            plugin_name = plugin.name,
            plugin_version = plugin.version
        );
        let auth = AuthCtx::new(token.secret());

        let response = self
            .plugin_service
            .create_plugin(&auth, plugin.0.into())
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }

    /// Registers a new library plugin
    #[oai(
        path = "/library-plugins/",
        method = "post",
        operation_id = "create_library_plugin"
    )]
    pub async fn create_library_plugin(
        &self,
        plugin: dto::LibraryPluginDefinitionCreation,
        token: GolemSecurityScheme,
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "create_library_plugin",
            plugin_name = plugin.name,
            plugin_version = plugin.version
        );
        let auth = AuthCtx::new(token.secret());

        let response = self
            .plugin_service
            .create_plugin(&auth, plugin.into())
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }

    /// Registers a new app plugin
    #[oai(
        path = "/app-plugins/",
        method = "post",
        operation_id = "create_app_plugin"
    )]
    pub async fn create_app_plugin(
        &self,
        plugin: dto::AppPluginDefinitionCreation,
        token: GolemSecurityScheme,
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "create_app_plugin",
            plugin_name = plugin.name,
            plugin_version = plugin.version
        );
        let auth = AuthCtx::new(token.secret());

        let response = self
            .plugin_service
            .create_plugin(&auth, plugin.into())
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }
}
