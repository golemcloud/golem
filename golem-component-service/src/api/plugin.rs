use crate::api::{ApiTags, ComponentError, Result};
use crate::service::plugin::CloudPluginService;
use cloud_common::auth::{CloudAuthCtx, GolemSecurityScheme};
use cloud_common::model::{CloudPluginOwner, CloudPluginScope};
use golem_common::model::error::ErrorBody;
use golem_common::model::plugin::PluginDefinition;
use golem_common::model::Empty;
use golem_common::recorded_http_api_request;
use golem_component_service_base::api::dto;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::OpenApi;
use std::sync::Arc;
use tracing::Instrument;

pub struct PluginApi {
    plugin_service: Arc<CloudPluginService>,
}

#[OpenApi(prefix_path = "/v1/plugins", tag = ApiTags::Plugin)]
impl PluginApi {
    pub fn new(plugin_service: Arc<CloudPluginService>) -> Self {
        Self { plugin_service }
    }

    /// Lists all the registered plugins (including all versions of each).
    #[oai(path = "/", method = "get", operation_id = "list_plugins")]
    pub async fn list_plugins(
        &self,
        scope: Query<Option<CloudPluginScope>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<PluginDefinition<CloudPluginOwner, CloudPluginScope>>>> {
        let record = recorded_http_api_request!("list_plugins",);
        let auth = CloudAuthCtx::new(token.secret());

        let response = if let Some(scope) = scope.0 {
            self.plugin_service
                .list_plugins_for_scope(&auth, &scope)
                .instrument(record.span.clone())
                .await
                .map_err(|e| e.into())
                .map(|response| Json(response.into_iter().collect()))
        } else {
            self.plugin_service
                .list_plugins(&auth)
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
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<PluginDefinition<CloudPluginOwner, CloudPluginScope>>>> {
        let record = recorded_http_api_request!("list_plugin_versions", plugin_name = name.0);
        let auth = CloudAuthCtx::new(token.secret());

        let response = self
            .plugin_service
            .list_plugin_versions(&auth, &name)
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
        plugin: Json<dto::PluginDefinitionCreation<CloudPluginScope>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "create_plugin",
            plugin_name = plugin.name,
            plugin_version = plugin.version
        );
        let auth = CloudAuthCtx::new(token.secret());

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
        plugin: dto::LibraryPluginDefinitionCreation<CloudPluginScope>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "create_library_plugin",
            plugin_name = plugin.name,
            plugin_version = plugin.version
        );
        let auth = CloudAuthCtx::new(token.secret());

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
        plugin: dto::AppPluginDefinitionCreation<CloudPluginScope>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "create_app_plugin",
            plugin_name = plugin.name,
            plugin_version = plugin.version
        );
        let auth = CloudAuthCtx::new(token.secret());

        let response = self
            .plugin_service
            .create_plugin(&auth, plugin.into())
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
        token: GolemSecurityScheme,
    ) -> Result<Json<PluginDefinition<CloudPluginOwner, CloudPluginScope>>> {
        let record = recorded_http_api_request!(
            "get_plugin",
            plugin_name = name.0,
            plugin_version = version.0
        );
        let auth = CloudAuthCtx::new(token.secret());

        let response = self
            .plugin_service
            .get(&auth, &name, &version)
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
        token: GolemSecurityScheme,
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "delete_plugin",
            plugin_name = name.0,
            plugin_version = version.0
        );
        let auth = CloudAuthCtx::new(token.secret());

        let response = self
            .plugin_service
            .delete(&auth, &name, &version)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }
}
