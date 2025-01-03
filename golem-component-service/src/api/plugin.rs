// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::api::{ComponentError, Result};
use golem_common::model::plugin::{
    DefaultPluginOwner, DefaultPluginScope, PluginDefinition, PluginDefinitionWithoutOwner,
};
use golem_common::model::Empty;
use golem_common::recorded_http_api_request;
use golem_component_service_base::service::plugin::PluginService;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::ErrorBody;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;
use std::sync::Arc;
use tracing::Instrument;

pub struct PluginApi {
    pub plugin_service:
        Arc<dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/plugins", tag = ApiTags::Plugin)]
impl PluginApi {
    /// Lists all the registered plugins (including all versions of each).
    #[oai(path = "/", method = "get", operation_id = "list_plugins")]
    pub async fn list_plugins(
        &self,
        scope: Query<Option<DefaultPluginScope>>,
    ) -> Result<Json<Vec<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>>>> {
        let record = recorded_http_api_request!("list_plugins",);

        let response = if let Some(scope) = scope.0 {
            self.plugin_service
                .list_plugins_for_scope(&DefaultPluginOwner, &scope, ())
                .instrument(record.span.clone())
                .await
                .map_err(|e| e.into())
                .map(|response| Json(response.into_iter().collect()))
        } else {
            self.plugin_service
                .list_plugins(&DefaultPluginOwner)
                .instrument(record.span.clone())
                .await
                .map_err(|e| e.into())
                .map(|response| Json(response.into_iter().collect()))
        };

        record.result(response)
    }

    /// Lists all the registered versions of a specific plugin identified by its name
    #[oai(path = "/:name", method = "get", operation_id = "list_plugin_versions")]
    pub async fn list_plugin_versions(
        &self,
        name: Path<String>,
    ) -> Result<Json<Vec<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>>>> {
        let record = recorded_http_api_request!("list_plugin_versions", plugin_name = name.0);

        let response = self
            .plugin_service
            .list_plugin_versions(&DefaultPluginOwner, &name)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|response| Json(response.into_iter().collect()));

        record.result(response)
    }

    /// Registers a new plugin
    #[oai(path = "/", method = "post", operation_id = "create_plugin")]
    pub async fn create_plugin(
        &self,
        plugin: Json<PluginDefinitionWithoutOwner<DefaultPluginScope>>,
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "create_plugin",
            plugin_name = plugin.name,
            plugin_version = plugin.version
        );

        let response = self
            .plugin_service
            .create_plugin(plugin.0.with_owner(DefaultPluginOwner))
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }

    /// Gets a registered plugin by its name and version
    #[oai(path = "/:name/:version", method = "get", operation_id = "get_plugin")]
    pub async fn get_plugin(
        &self,
        name: Path<String>,
        version: Path<String>,
    ) -> Result<Json<PluginDefinition<DefaultPluginOwner, DefaultPluginScope>>> {
        let record = recorded_http_api_request!(
            "get_plugin",
            plugin_name = name.0,
            plugin_version = version.0
        );

        let response = self
            .plugin_service
            .get(&DefaultPluginOwner, &name, &version)
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
        path = "/:name/:version",
        method = "delete",
        operation_id = "delete_plugin"
    )]
    pub async fn delete_plugin(
        &self,
        name: Path<String>,
        version: Path<String>,
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "delete_plugin",
            plugin_name = name.0,
            plugin_version = version.0
        );

        let response = self
            .plugin_service
            .delete(&DefaultPluginOwner, &name, &version)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }
}
