// Copyright 2024 Golem Cloud
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

use futures_util::TryStreamExt;
use golem_common::model::ComponentId;
use golem_component_service_base::service::component::{
    ComponentError as ComponentServiceError, ComponentService,
};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::auth::DefaultNamespace;
use golem_service_base::model::*;
use poem::error::ReadBodyError;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::types::multipart::Upload;
use poem_openapi::*;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::Instrument;

use golem_common::metrics::api::TraceErrorKind;
use golem_common::recorded_http_api_request;

#[derive(ApiResponse, Debug, Clone)]
pub enum ComponentError {
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl TraceErrorKind for ComponentError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            ComponentError::BadRequest(_) => "BadRequest",
            ComponentError::NotFound(_) => "NotFound",
            ComponentError::AlreadyExists(_) => "AlreadyExists",
            ComponentError::LimitExceeded(_) => "LimitExceeded",
            ComponentError::Unauthorized(_) => "Unauthorized",
            ComponentError::InternalError(_) => "InternalError",
        }
    }
}

#[derive(Multipart)]
pub struct UploadPayload {
    name: ComponentName,
    component: Upload,
}

type Result<T> = std::result::Result<T, ComponentError>;

impl From<ComponentServiceError> for ComponentError {
    fn from(error: ComponentServiceError) -> Self {
        match error {
            ComponentServiceError::UnknownComponentId(_)
            | ComponentServiceError::UnknownVersionedComponentId(_) => {
                ComponentError::NotFound(Json(ErrorBody {
                    error: error.to_string(),
                }))
            }
            ComponentServiceError::AlreadyExists(_) => {
                ComponentError::AlreadyExists(Json(ErrorBody {
                    error: error.to_string(),
                }))
            }
            ComponentServiceError::Internal(error) => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: error.to_string(),
                }))
            }
            ComponentServiceError::ComponentProcessingError(error) => {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec![error.to_string()],
                }))
            }
        }
    }
}

impl From<ReadBodyError> for ComponentError {
    fn from(value: ReadBodyError) -> Self {
        ComponentError::InternalError(Json(ErrorBody {
            error: value.to_string(),
        }))
    }
}

impl From<std::io::Error> for ComponentError {
    fn from(value: std::io::Error) -> Self {
        ComponentError::InternalError(Json(ErrorBody {
            error: value.to_string(),
        }))
    }
}

pub struct ComponentApi {
    pub component_service: Arc<dyn ComponentService<DefaultNamespace> + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/components", tag = ApiTags::Component)]
impl ComponentApi {
    /// Create a new component
    ///
    /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    #[oai(path = "/", method = "post", operation_id = "create_component")]
    async fn create_component(&self, payload: UploadPayload) -> Result<Json<Component>> {
        let record =
            recorded_http_api_request!("create_component", component_name = payload.name.0);
        let response = {
            let data = payload.component.into_vec().await?;
            let component_name = payload.name;
            self.component_service
                .create(
                    &ComponentId::new_v4(),
                    &component_name,
                    data,
                    &DefaultNamespace::default(),
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
        operation_id = "update_component"
    )]
    async fn update_component(
        &self,
        component_id: Path<ComponentId>,
        wasm: Binary<Body>,
    ) -> Result<Json<Component>> {
        let record = recorded_http_api_request!(
            "update_component",
            component_id = component_id.0.to_string()
        );
        let response = {
            let data = wasm.0.into_vec().await?;
            self.component_service
                .update(&component_id.0, data, &DefaultNamespace::default())
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
            .download_stream(&component_id.0, version.0, &DefaultNamespace::default())
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
            .get(&component_id.0, &DefaultNamespace::default())
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
        #[oai(name = "component_id")] component_id: Path<ComponentId>,
        #[oai(name = "version")] version: Path<String>,
    ) -> Result<Json<Component>> {
        let record = recorded_http_api_request!(
            "get_component_metadata",
            component_id = component_id.0.to_string(),
            version = version.0,
        );

        let response = {
            let version_int = version.0.parse::<u64>().map_err(|_| {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec!["Invalid version".to_string()],
                }))
            })?;

            let versioned_component_id = VersionedComponentId {
                component_id: component_id.0,
                version: version_int,
            };

            self.component_service
                .get_by_version(&versioned_component_id, &DefaultNamespace::default())
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
            .get_latest_version(&component_id.0, &DefaultNamespace::default())
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
            .find_by_name(component_name.0, &DefaultNamespace::default())
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(|components| Json(components.into_iter().map(|c| c.into()).collect()));

        record.result(response)
    }
}
