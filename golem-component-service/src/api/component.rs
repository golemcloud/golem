use super::dto;
use super::dto::CloudApiMapper;
use crate::api::{ApiTags, ComponentError, Result};
use crate::model::{ComponentQuery, ComponentSearch};
use crate::service::component::CloudComponentService;
use cloud_common::auth::{CloudAuthCtx, GolemSecurityScheme};
use futures_util::{stream, StreamExt, TryStreamExt};
use golem_common::model::component::VersionedComponentId;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::model::plugin::{PluginInstallationCreation, PluginInstallationUpdate};
use golem_common::model::{
    ComponentFilePathWithPermissionsList, Empty, PluginInstallationId, ProjectId,
};
use golem_common::model::{ComponentId, ComponentType};
use golem_common::recorded_http_api_request;
use golem_component_service_base::model::{
    BatchPluginInstallationUpdates, ComponentEnv, DynamicLinking,
    InitialComponentFilesArchiveAndPermissions, UpdatePayload,
};
use golem_service_base::model::ComponentName;
use golem_service_base::poem::TempFileUpload;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::*;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::Instrument;

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct UploadPayload {
    query: JsonField<ComponentQuery>,
    component: Upload,
    component_type: Option<ComponentType>,
    files_permissions: Option<ComponentFilePathWithPermissionsList>,
    files: Option<TempFileUpload>,
    dynamic_linking: Option<JsonField<DynamicLinking>>,
    env: Option<JsonField<ComponentEnv>>,
}

pub struct ComponentApi {
    component_service: Arc<CloudComponentService>,
    api_mapper: Arc<dyn CloudApiMapper>,
}

#[OpenApi(prefix_path = "/v1/components", tag = ApiTags::Component)]
impl ComponentApi {
    pub fn new(
        component_service: Arc<CloudComponentService>,
        api_mapper: Arc<dyn CloudApiMapper>,
    ) -> Self {
        Self {
            component_service,
            api_mapper,
        }
    }

    /// Get the metadata for all component versions
    ///
    /// Each component can have multiple versions. Every time a new WASM is uploaded for a given component id, that creates a new version.
    /// This endpoint returns a list of all versions for the component id provided as part of the URL. Each element of the response describes a single version of a component, but does not contain the binary (WASM) itself:
    ///
    /// - `versionedComponentId` associates a specific version with the component id
    /// - `componentName` is the human-readable name of the component
    /// - `componentSize` is the WASM binary's size in bytes
    /// - `metadata` contains information extracted from the WASM itself
    /// - `metadata.exports` is a list of exported functions, including their parameter's and return value's types
    /// - `metadata.producers` is a list of producer information added by tooling, each consisting of a list of fields associating one or more values to a given key. This contains information about what compilers and other WASM related tools were used to construct the Golem component.
    #[oai(
        path = "/:component_id",
        method = "get",
        operation_id = "get_component_metadata_all_versions"
    )]
    async fn get_component_metadata_all_versions(
        &self,
        component_id: Path<ComponentId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<dto::Component>>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "get_component_metadata_all_versions",
            component_id = component_id.0.to_string()
        );
        let response = self
            .get_component_metadata_all_versions_internal(&component_id.0, auth)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn get_component_metadata_all_versions_internal(
        &self,
        component_id: &ComponentId,
        auth: CloudAuthCtx,
    ) -> Result<Json<Vec<dto::Component>>> {
        let components = self.component_service.get(component_id, &auth).await?;

        let converted = stream::iter(components)
            .then(|c| self.api_mapper.convert_component(c))
            .try_collect::<Vec<_>>()
            .await?;

        Ok(Json(converted))
    }

    /// Update a component
    #[oai(
        path = "/:component_id/upload",
        method = "put",
        operation_id = "upload_component"
    )]
    async fn upload_component(
        &self,
        component_id: Path<ComponentId>,
        wasm: Binary<Body>,
        /// Type of the new version of the component - if not specified, the type of the previous version
        /// is used.
        component_type: Query<Option<ComponentType>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<dto::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "upload_component",
            component_id = component_id.0.to_string()
        );
        let response = self
            .upload_component_internal(component_id.0, wasm.0, component_type.0, auth)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn upload_component_internal(
        &self,
        component_id: ComponentId,
        wasm: Body,
        component_type: Option<ComponentType>,
        auth: CloudAuthCtx,
    ) -> Result<Json<dto::Component>> {
        let data = wasm.into_vec().await?;
        let response = self
            .component_service
            .update(
                &component_id,
                component_type,
                data,
                None,
                HashMap::new(),
                &auth,
                HashMap::new(),
            )
            .await?;

        Ok(Json(self.api_mapper.convert_component(response).await?))
    }

    /// Update a component
    #[oai(
        path = "/:component_id/updates",
        method = "post",
        operation_id = "update_component"
    )]
    async fn update_component(
        &self,
        component_id: Path<ComponentId>,
        payload: UpdatePayload,
        token: GolemSecurityScheme,
    ) -> Result<Json<dto::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "update_component",
            component_id = component_id.0.to_string()
        );
        let response = self
            .update_component_internal(component_id.0, payload, auth)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn update_component_internal(
        &self,
        component_id: ComponentId,
        payload: UpdatePayload,
        auth: CloudAuthCtx,
    ) -> Result<Json<dto::Component>> {
        let data = payload.component.into_vec().await?;
        let files_file = payload.files.map(|f| f.into_file());

        let files = files_file
            .zip(payload.files_permissions)
            .map(
                |(archive, permissions)| InitialComponentFilesArchiveAndPermissions {
                    archive,
                    files: permissions.values,
                },
            );

        let env = payload.env.map(|e| e.0.key_values).unwrap_or_default();

        let response = self
            .component_service
            .update(
                &component_id,
                payload.component_type,
                data,
                files,
                payload
                    .dynamic_linking
                    .unwrap_or_default()
                    .0
                    .dynamic_linking,
                &auth,
                env,
            )
            .await?;

        Ok(Json(self.api_mapper.convert_component(response).await?))
    }

    /// Create a new component
    ///
    /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    /// If the component type is not specified, it will be considered as a `Durable` component.
    #[oai(path = "/", method = "post", operation_id = "create_component")]
    async fn create_component(
        &self,
        payload: UploadPayload,
        token: GolemSecurityScheme,
    ) -> Result<Json<dto::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "create_component",
            component_name = payload.query.0.component_name.to_string(),
            project_id = payload.query.0.project_id.as_ref().map(|v| v.to_string()),
        );
        let response = self
            .create_component_internal(payload, auth)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn create_component_internal(
        &self,
        payload: UploadPayload,
        auth: CloudAuthCtx,
    ) -> Result<Json<dto::Component>> {
        let data = payload.component.into_vec().await?;
        let component_name = payload.query.0.component_name;
        let project_id = payload.query.0.project_id;

        let files_file = payload.files.map(|f| f.into_file());

        let files = files_file
            .zip(payload.files_permissions)
            .map(
                |(archive, permissions)| InitialComponentFilesArchiveAndPermissions {
                    archive,
                    files: permissions.values,
                },
            );

        let env = payload.env.map(|e| e.0.key_values).unwrap_or_default();

        let response = self
            .component_service
            .create(
                project_id,
                &component_name,
                payload.component_type.unwrap_or(ComponentType::Durable),
                data,
                files,
                payload
                    .dynamic_linking
                    .unwrap_or_default()
                    .0
                    .dynamic_linking,
                &auth,
                env,
            )
            .await?;

        Ok(Json(self.api_mapper.convert_component(response).await?))
    }

    /// Download a component
    ///
    /// Downloads a specific version of the component's WASM.
    #[oai(
        path = "/:component_id/download",
        method = "get",
        operation_id = "download_component"
    )]
    async fn download_component(
        &self,
        component_id: Path<ComponentId>,
        version: Query<Option<u64>>,
        token: GolemSecurityScheme,
    ) -> Result<Binary<Body>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "download_component",
            component_id = component_id.0.to_string(),
            version = version.0.map(|v| v.to_string())
        );
        let response = self
            .component_service
            .download_stream(&component_id.0, version.0, &auth)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|bytes| {
                Binary(Body::from_bytes_stream(
                    bytes.map_err(|e| std::io::Error::other(e.to_string())),
                ))
            });
        record.result(response)
    }

    /// Get the version of a given component
    ///
    /// Gets the version of a component.
    #[oai(
        path = "/:component_id/versions/:version",
        method = "get",
        operation_id = "get_component_metadata"
    )]
    async fn get_component_metadata(
        &self,
        #[oai(name = "component_id")] component_id: Path<ComponentId>,
        #[oai(name = "version")] version: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<dto::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "get_component_metadata",
            component_id = component_id.0.to_string(),
            version = version.0,
        );

        let response = self
            .get_component_metadata_internal(component_id.0, version.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_component_metadata_internal(
        &self,
        component_id: ComponentId,
        version: String,
        auth: CloudAuthCtx,
    ) -> Result<Json<dto::Component>> {
        let version_int = Self::parse_version_path_segment(&version)?;

        let versioned_component_id = VersionedComponentId {
            component_id,
            version: version_int,
        };

        let response = self
            .component_service
            .get_by_version(&versioned_component_id, &auth)
            .await?;

        match response {
            Some(component) => {
                let converted = self.api_mapper.convert_component(component).await?;
                Ok(Json(converted))
            }
            None => Err(ComponentError::NotFound(Json(ErrorBody {
                error: "Component not found".to_string(),
            }))),
        }
    }

    /// Get the latest version of a given component
    ///
    /// Gets the latest version of a component.
    #[oai(
        path = "/:component_id/latest",
        method = "get",
        operation_id = "get_latest_component_metadata"
    )]
    async fn get_latest_component_metadata(
        &self,
        component_id: Path<ComponentId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<dto::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "get_latest_component_metadata",
            component_id = component_id.0.to_string()
        );

        let response = self
            .get_latest_component_metadata_internal(component_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_latest_component_metadata_internal(
        &self,
        component_id: ComponentId,
        auth: CloudAuthCtx,
    ) -> Result<Json<dto::Component>> {
        let response = self
            .component_service
            .get_latest_version(&component_id, &auth)
            .await?;

        match response {
            Some(component) => {
                let converted = self.api_mapper.convert_component(component).await?;
                Ok(Json(converted))
            }
            None => Err(ComponentError::NotFound(Json(ErrorBody {
                error: "Component not found".to_string(),
            }))),
        }
    }

    /// Get all components
    ///
    /// Gets all components, optionally filtered by project and/or component name.
    #[oai(path = "/", method = "get", operation_id = "get_components")]
    async fn get_components(
        &self,
        /// Project ID to filter by
        #[oai(name = "project-id")]
        project_id: Query<Option<ProjectId>>,
        /// Component name to filter by
        #[oai(name = "component-name")]
        component_name: Query<Option<ComponentName>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<dto::Component>>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "get_components",
            component_name = component_name.0.as_ref().map(|v| v.0.clone()),
            project_id = project_id.0.as_ref().map(|v| v.to_string()),
        );

        let response = self
            .get_components_internal(project_id.0, component_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_components_internal(
        &self,
        project_id: Option<ProjectId>,
        component_name: Option<ComponentName>,
        auth: CloudAuthCtx,
    ) -> Result<Json<Vec<dto::Component>>> {
        let components = self
            .component_service
            .find_by_project_and_name(project_id, component_name, &auth)
            .await?;

        let converted = stream::iter(components)
            .then(|c| self.api_mapper.convert_component(c))
            .try_collect::<Vec<_>>()
            .await?;

        Ok(Json(converted))
    }

    #[oai(path = "/search", method = "post", operation_id = "search_components")]
    async fn search_components(
        &self,
        components_search: Json<ComponentSearch>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<dto::Component>>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "search_components",
            search_components = components_search
                .components
                .iter()
                .map(|query| query.name.0.clone())
                .collect::<Vec<_>>()
                .join(", ")
        );

        let response = self
            .search_components_internal(components_search.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn search_components_internal(
        &self,
        search_query: ComponentSearch,
        auth: CloudAuthCtx,
    ) -> Result<Json<Vec<dto::Component>>> {
        let component_by_name_and_versions = search_query
            .components
            .into_iter()
            .map(|query| query.into())
            .collect::<Vec<_>>();

        let components = self
            .component_service
            .find_by_project_and_names(
                search_query.project_id,
                component_by_name_and_versions,
                &auth,
            )
            .await?;

        let mut converted = Vec::new();
        for component in components {
            converted.push(self.api_mapper.convert_component(component).await?);
        }

        Ok(Json(converted))
    }

    /// Gets the list of plugins installed for the given component version
    #[oai(
        path = "/:component_id/versions/:version/plugins/installs",
        method = "get",
        operation_id = "get_installed_plugins"
    )]
    async fn get_installed_plugins(
        &self,
        component_id: Path<ComponentId>,
        version: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<dto::PluginInstallation>>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "get_installed_plugins",
            component_id = component_id.0.to_string(),
            version = version.0,
        );

        let response = self
            .get_installed_plugins_internal(component_id.0, version.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_installed_plugins_internal(
        &self,
        component_id: ComponentId,
        version: String,
        auth: CloudAuthCtx,
    ) -> Result<Json<Vec<dto::PluginInstallation>>> {
        let version_int = Self::parse_version_path_segment(&version)?;

        let (owner, installations) = self
            .component_service
            .get_plugin_installations_for_component(&auth, &component_id, version_int)
            .await?;

        let converted = stream::iter(installations)
            .then(|pi| self.api_mapper.convert_plugin_installation(&owner, pi))
            .try_collect::<Vec<_>>()
            .await?;

        Ok(Json(converted))
    }

    /// Installs a new plugin for this component
    #[oai(
        path = "/:component_id/latest/plugins/installs",
        method = "post",
        operation_id = "install_plugin"
    )]
    async fn install_plugin(
        &self,
        component_id: Path<ComponentId>,
        plugin: Json<PluginInstallationCreation>,
        token: GolemSecurityScheme,
    ) -> Result<Json<dto::PluginInstallation>> {
        let auth = CloudAuthCtx::new(token.secret());

        let record = recorded_http_api_request!(
            "install_plugin",
            component_id = component_id.0.to_string(),
            plugin_name = plugin.name.clone(),
            plugin_version = plugin.version.clone()
        );

        let response = self
            .install_plugin_internal(component_id.0, plugin.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn install_plugin_internal(
        &self,
        component_id: ComponentId,
        plugin: PluginInstallationCreation,
        auth: CloudAuthCtx,
    ) -> Result<Json<dto::PluginInstallation>> {
        let (owner, installation) = self
            .component_service
            .create_plugin_installation_for_component(&auth, &component_id, plugin)
            .await?;

        Ok(Json(
            self.api_mapper
                .convert_plugin_installation(&owner, installation)
                .await?,
        ))
    }

    /// Updates the priority or parameters of a plugin installation
    #[oai(
        path = "/:component_id/versions/latest/plugins/installs/:installation_id",
        method = "put",
        operation_id = "update_installed_plugin"
    )]
    async fn update_installed_plugin(
        &self,
        component_id: Path<ComponentId>,
        installation_id: Path<PluginInstallationId>,
        update: Json<PluginInstallationUpdate>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Empty>> {
        let auth = CloudAuthCtx::new(token.secret());

        let record = recorded_http_api_request!(
            "update_installed_plugin",
            component_id = component_id.0.to_string(),
            installation_id = installation_id.0.to_string()
        );

        let response = self
            .component_service
            .update_plugin_installation_for_component(
                &auth,
                &installation_id.0,
                &component_id.0,
                update.0,
            )
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }

    /// Uninstalls a plugin from this component
    #[oai(
        path = "/:component_id/latest/plugins/installs/:installation_id",
        method = "delete",
        operation_id = "uninstall_plugin"
    )]
    async fn uninstall_plugin(
        &self,
        component_id: Path<ComponentId>,
        installation_id: Path<PluginInstallationId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Empty>> {
        let auth = CloudAuthCtx::new(token.secret());

        let record = recorded_http_api_request!(
            "uninstall_plugin",
            component_id = component_id.0.to_string(),
            installation_id = installation_id.0.to_string()
        );

        let response = self
            .component_service
            .delete_plugin_installation_for_component(&auth, &installation_id.0, &component_id.0)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }

    /// Applies a batch of changes to the installed plugins of a component
    #[oai(
        path = "/:component_id/versions/latest/plugins/installs/batch",
        method = "post",
        operation_id = "bath_update_installed_plugins"
    )]
    async fn bath_update_installed_plugins(
        &self,
        component_id: Path<ComponentId>,
        updates: Json<BatchPluginInstallationUpdates>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Empty>> {
        let auth = CloudAuthCtx::new(token.secret());

        let record = recorded_http_api_request!(
            "batch_update_installed_plugins",
            component_id = component_id.0.to_string(),
        );

        let response = self
            .batch_update_installed_plugins_internal(component_id.0, updates.0, auth)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn batch_update_installed_plugins_internal(
        &self,
        component_id: ComponentId,
        updates: BatchPluginInstallationUpdates,
        auth: CloudAuthCtx,
    ) -> Result<Json<Empty>> {
        self.component_service
            .batch_update_plugin_installations_for_component(&auth, &component_id, &updates.actions)
            .await?;
        Ok(Json(Empty {}))
    }

    /// Download file in a Component
    #[oai(
        path = "/:component_id/versions/:version/file-contents/:file_path",
        method = "get",
        operation_id = "download_component_file"
    )]
    async fn download_component_file(
        &self,
        component_id: Path<ComponentId>,
        version: Path<String>,
        file_path: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Binary<Body>> {
        let auth = CloudAuthCtx::new(token.secret());

        let record = recorded_http_api_request!(
            "download_component_file",
            component_id = component_id.0.to_string(),
            version = version.0,
            file_path = file_path.0.as_str()
        );

        let response = self
            .download_component_file_internal(component_id.0, version.0, file_path.0, auth)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn download_component_file_internal(
        &self,
        component_id: ComponentId,
        version: String,
        file_path: String,
        auth: CloudAuthCtx,
    ) -> Result<Binary<Body>> {
        let version_int = Self::parse_version_path_segment(&version)?;

        self.component_service
            .get_file_contents(&auth, &component_id, version_int, file_path.as_str())
            .await
            .map_err(|e| e.into())
            .map(|bytes| {
                Binary(Body::from_bytes_stream(
                    bytes.map_err(|e| std::io::Error::other(e.to_string())),
                ))
            })
    }

    fn parse_version_path_segment(version: &str) -> Result<u64> {
        version.parse::<u64>().map_err(|_| {
            ComponentError::BadRequest(Json(ErrorsBody {
                errors: vec!["Invalid version".to_string()],
            }))
        })
    }
}
