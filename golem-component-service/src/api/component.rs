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
use futures_util::TryStreamExt;
use golem_common::model::component::DefaultComponentOwner;
use golem_common::model::plugin::{
    DefaultPluginOwner, DefaultPluginScope, PluginInstallation, PluginInstallationCreation,
    PluginInstallationUpdate,
};
use golem_common::model::ComponentFilePathWithPermissionsList;
use golem_common::model::{ComponentId, ComponentType, Empty, PluginInstallationId};
use golem_common::recorded_http_api_request;
use golem_component_service_base::model::{
    InitialComponentFilesArchiveAndPermissions, UpdatePayload,
};
use golem_component_service_base::service::component::ComponentService;
use golem_component_service_base::service::plugin::{PluginError, PluginService};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::*;
use golem_service_base::poem::TempFileUpload;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::types::multipart::Upload;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct ComponentApi {
    pub component_service: Arc<dyn ComponentService<DefaultComponentOwner> + Sync + Send>,
    pub plugin_service:
        Arc<dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/components", tag = ApiTags::Component)]
impl ComponentApi {
    /// Create a new component
    ///
    /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    /// If the component type is not specified, it will be considered as a `Durable` component.
    #[oai(path = "/", method = "post", operation_id = "create_component")]
    async fn create_component(&self, payload: UploadPayload) -> Result<Json<Component>> {
        let component_id = ComponentId::new_v4();
        let record = recorded_http_api_request!(
            "create_component",
            component_name = payload.name.0,
            component_id = component_id.to_string()
        );
        let response = {
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

            let component_name = payload.name;
            self.component_service
                .create(
                    &component_id,
                    &component_name,
                    payload.component_type.unwrap_or(ComponentType::Durable),
                    data,
                    files,
                    vec![],
                    &DefaultComponentOwner,
                )
                .instrument(record.span.clone())
                .await
                .map_err(|e| e.into())
                .map(|response| Json(response.into()))
        };
        record.result(response)
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
    ) -> Result<Json<Component>> {
        let record = recorded_http_api_request!(
            "upload_component",
            component_id = component_id.0.to_string()
        );

        let response = {
            let data = wasm.0.into_vec().await?;
            self.component_service
                .update(
                    &component_id.0,
                    data,
                    component_type.0,
                    None,
                    &DefaultComponentOwner,
                )
                .instrument(record.span.clone())
                .await
                .map_err(|e| e.into())
                .map(|response| Json(response.into()))
        };
        record.result(response)
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
    ) -> Result<Json<Component>> {
        let record = recorded_http_api_request!(
            "update_component",
            component_id = component_id.0.to_string()
        );
        let response = {
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

            self.component_service
                .update(
                    &component_id.0,
                    data,
                    payload.component_type,
                    files,
                    &DefaultComponentOwner,
                )
                .instrument(record.span.clone())
                .await
                .map_err(|e| e.into())
                .map(|response| Json(response.into()))
        };
        record.result(response)
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
    ) -> Result<Binary<Body>> {
        let record = recorded_http_api_request!(
            "download_component",
            component_id = component_id.0.to_string(),
            version = version.0.map(|v| v.to_string())
        );
        let response = self
            .component_service
            .download_stream(&component_id.0, version.0, &DefaultComponentOwner)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|bytes| {
                Binary(Body::from_bytes_stream(bytes.map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
                })))
            });
        record.result(response)
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
    ) -> Result<Json<Vec<Component>>> {
        let record = recorded_http_api_request!(
            "get_component_metadata_all_versions",
            component_id = component_id.0.to_string()
        );

        let response = self
            .component_service
            .get(&component_id.0, &DefaultComponentOwner)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|response| Json(response.into_iter().map(|c| c.into()).collect()));

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
        component_id: Path<ComponentId>,
        version: Path<String>,
    ) -> Result<Json<Component>> {
        let record = recorded_http_api_request!(
            "get_component_metadata",
            component_id = component_id.0.to_string(),
            version = version.0,
        );

        let response = {
            let version_int = Self::parse_version_path_segment(&version.0)?;

            let versioned_component_id = VersionedComponentId {
                component_id: component_id.0,
                version: version_int,
            };

            self.component_service
                .get_by_version(&versioned_component_id, &DefaultComponentOwner)
                .instrument(record.span.clone())
                .await
                .map_err(|e| e.into())
                .and_then(|response| match response {
                    Some(component) => Ok(Json(component.into())),
                    None => Err(ComponentError::NotFound(Json(ErrorBody {
                        error: "Component not found".to_string(),
                    }))),
                })
        };

        record.result(response)
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
    ) -> Result<Json<Component>> {
        let record = recorded_http_api_request!(
            "get_latest_component_metadata",
            component_id = component_id.0.to_string()
        );

        let response = self
            .component_service
            .get_latest_version(&component_id.0, &DefaultComponentOwner)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .and_then(|response| match response {
                Some(component) => Ok(Json(component.into())),
                None => Err(ComponentError::NotFound(Json(ErrorBody {
                    error: "Component not found".to_string(),
                }))),
            });

        record.result(response)
    }

    /// Get all components
    ///
    /// Gets all components, optionally filtered by component name.
    #[oai(path = "/", method = "get", operation_id = "get_components")]
    async fn get_components(
        &self,
        #[oai(name = "component-name")] component_name: Query<Option<ComponentName>>,
    ) -> Result<Json<Vec<Component>>> {
        let record = recorded_http_api_request!(
            "get_components",
            component_name = component_name.0.as_ref().map(|n| n.0.clone())
        );

        let response = self
            .component_service
            .find_by_name(component_name.0, &DefaultComponentOwner)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|components| Json(components.into_iter().map(|c| c.into()).collect()));

        record.result(response)
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
    ) -> Result<Json<Vec<PluginInstallation>>> {
        let record = recorded_http_api_request!(
            "get_installed_plugins",
            component_id = component_id.0.to_string(),
            version = version.0,
        );

        let version_int = Self::parse_version_path_segment(&version.0)?;

        let response = self
            .component_service
            .get_plugin_installations_for_component(
                &DefaultComponentOwner,
                &component_id.0,
                version_int,
            )
            .await
            .map_err(|e| e.into())
            .map(Json);

        record.result(response)
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
    ) -> Result<Json<PluginInstallation>> {
        let record = recorded_http_api_request!(
            "install_plugin",
            component_id = component_id.0.to_string(),
            plugin_name = plugin.name.clone(),
            plugin_version = plugin.version.clone()
        );

        let plugin_definition = self
            .plugin_service
            .get(&DefaultPluginOwner, &plugin.name, &plugin.version)
            .await?;

        let response = if let Some(plugin_definition) = plugin_definition {
            if plugin_definition.scope.valid_in_component(&component_id.0) {
                self.component_service
                    .create_plugin_installation_for_component(
                        &DefaultComponentOwner,
                        &component_id.0,
                        plugin.0,
                    )
                    .await
            } else {
                Err(PluginError::InvalidScope {
                    plugin_name: plugin.name.clone(),
                    plugin_version: plugin.version.clone(),
                    details: format!("not available for component {}", component_id.0),
                })
            }
        } else {
            Err(PluginError::PluginNotFound {
                plugin_name: plugin.name.clone(),
                plugin_version: plugin.version.clone(),
            })
        };

        record.result(response.map_err(|e| e.into()).map(Json))
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
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "update_installed_plugin",
            component_id = component_id.0.to_string(),
            installation_id = installation_id.0.to_string()
        );

        let response = self
            .component_service
            .update_plugin_installation_for_component(
                &DefaultComponentOwner,
                &installation_id.0,
                &component_id.0,
                update.0,
            )
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
    ) -> Result<Json<Empty>> {
        let record = recorded_http_api_request!(
            "uninstall_plugin",
            component_id = component_id.0.to_string(),
            installation_id = installation_id.0.to_string()
        );

        let response = self
            .component_service
            .delete_plugin_installation_for_component(
                &DefaultComponentOwner,
                &installation_id.0,
                &component_id.0,
            )
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }

    fn parse_version_path_segment(version: &str) -> Result<u64> {
        version.parse::<u64>().map_err(|_| {
            ComponentError::BadRequest(Json(ErrorsBody {
                errors: vec!["Invalid version".to_string()],
            }))
        })
    }
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct UploadPayload {
    name: ComponentName,
    component_type: Option<ComponentType>,
    component: Upload,
    files_permissions: Option<ComponentFilePathWithPermissionsList>,
    files: Option<TempFileUpload>,
}
