use futures_util::TryStreamExt;
use std::sync::Arc;

use cloud_common::auth::GolemSecurityScheme;
use golem_common::model::ComponentId;
use golem_common::model::ProjectId;
use poem::error::ReadBodyError;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::*;

use crate::api::ApiTags;
use crate::model::*;
use crate::service::auth::CloudAuthCtx;
use crate::service::component::{ComponentError as ComponentServiceError, ComponentService};
use golem_service_base::model::*;

#[derive(ApiResponse)]
pub enum ComponentError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Maximum number of components exceeded
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    /// Component not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    /// Component already exists
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

#[derive(Multipart)]
pub struct UploadPayload {
    query: JsonField<ComponentQuery>,
    component: Upload,
}

type Result<T> = std::result::Result<T, ComponentError>;

impl From<ComponentServiceError> for ComponentError {
    fn from(value: ComponentServiceError) -> Self {
        match value {
            ComponentServiceError::Unauthorized(error) => {
                ComponentError::Unauthorized(Json(ErrorBody { error }))
            }
            ComponentServiceError::ComponentProcessing(error) => {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec![error.to_string()],
                }))
            }
            ComponentServiceError::UnknownComponentId(_)
            | ComponentServiceError::UnknownVersionedComponentId(_)
            | ComponentServiceError::UnknownProject(_) => {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_string()],
                }))
            }
            ComponentServiceError::LimitExceeded(error) => {
                ComponentError::LimitExceeded(Json(ErrorBody { error }))
            }
            ComponentServiceError::AlreadyExists(_) => {
                ComponentError::AlreadyExists(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
            ComponentServiceError::Internal(error) => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: error.to_string(),
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
    component_service: Arc<dyn ComponentService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/components", tag = ApiTags::Component)]
impl ComponentApi {
    pub fn new(component_service: Arc<dyn ComponentService + Sync + Send>) -> Self {
        Self { component_service }
    }

    /// Get the metadata for all component versions
    ///
    /// Each component can have multiple versions. Every time a new WASM is uploaded for a given component id, that creates a new version.
    /// This endpoint returns a list of all versions for the component id provided as part of the URL. Each element of the response describes a single version of a component, but does not contain the binary (WASM) itself:
    ///
    /// - `versionedComponentId` associates a specific version with the component id
    /// - `userComponentId` and protectedComponentId are implementation details, not used elsewhere on the public API
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
    ) -> Result<Json<Vec<crate::model::Component>>> {
        let auth = CloudAuthCtx::new(token.secret());
        let response = self.component_service.get(&component_id.0, &auth).await?;
        Ok(Json(response))
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
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
        let data = wasm.0.into_vec().await?;
        let response = self
            .component_service
            .update(&component_id.0, data, &auth)
            .await?;
        Ok(Json(response))
    }

    /// Create a new component
    ///
    /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    #[oai(path = "/", method = "post", operation_id = "create_component")]
    async fn create_component(
        &self,
        payload: UploadPayload,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
        let data = payload.component.into_vec().await?;
        let component_name = payload.query.0.component_name;
        let project_id = payload.query.0.project_id;
        let response = self
            .component_service
            .create(project_id, &component_name, data, &auth)
            .await?;
        Ok(Json(response))
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
        let bytes = self
            .component_service
            .download_stream(&component_id.0, version.0, &auth)
            .await?;
        Ok(Binary(Body::from_bytes_stream(bytes.map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        }))))
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
    ) -> Result<Json<crate::model::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
        let version_int = match version.0.parse::<u64>() {
            Ok(v) => v,
            Err(_) => {
                return Err(ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec!["Invalid version".to_string()],
                })))
            }
        };

        let versioned_component_id = VersionedComponentId {
            component_id: component_id.0,
            version: version_int,
        };

        let response = self
            .component_service
            .get_by_version(&versioned_component_id, &auth)
            .await?;

        match response {
            Some(component) => Ok(Json(component)),
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
    ) -> Result<Json<crate::model::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
        let response = self
            .component_service
            .get_latest_version(&component_id.0, &auth)
            .await?;

        match response {
            Some(component) => Ok(Json(component)),
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
    ) -> Result<Json<Vec<crate::model::Component>>> {
        let auth = CloudAuthCtx::new(token.secret());
        let response = self
            .component_service
            .find_by_project_and_name(project_id.0, component_name.0, &auth)
            .await?;

        Ok(Json(response))
    }
}
