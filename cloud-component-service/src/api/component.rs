use crate::api::ApiTags;
use crate::model::*;
use crate::service::auth::CloudAuthCtx;
use crate::service::component::{ComponentError as ComponentServiceError, ComponentService};
use cloud_common::auth::GolemSecurityScheme;
use futures_util::TryStreamExt;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::ComponentId;
use golem_common::model::ProjectId;
use golem_common::recorded_http_api_request;
use golem_service_base::model::*;
use poem::error::ReadBodyError;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

#[derive(ApiResponse, Debug, Clone)]
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
    query: JsonField<ComponentQuery>,
    component: Upload,
}

type Result<T> = std::result::Result<T, ComponentError>;

impl From<ComponentServiceError> for ComponentError {
    fn from(value: ComponentServiceError) -> Self {
        match value {
            ComponentServiceError::Unauthorized(_) => {
                ComponentError::Unauthorized(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
            ComponentServiceError::ComponentProcessing(_) => {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_string()],
                }))
            }
            ComponentServiceError::UnknownComponentId(_)
            | ComponentServiceError::UnknownVersionedComponentId(_)
            | ComponentServiceError::UnknownProject(_) => {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_string()],
                }))
            }
            ComponentServiceError::LimitExceeded(_) => {
                ComponentError::LimitExceeded(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
            ComponentServiceError::AlreadyExists(_) => {
                ComponentError::AlreadyExists(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
            ComponentServiceError::Internal(_) => ComponentError::InternalError(Json(ErrorBody {
                error: value.to_string(),
            })),
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

#[OpenApi(prefix_path = "/v1/components", tag = ApiTags::Component)]
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
        let record = recorded_http_api_request!(
            "update_component",
            component_id = component_id.0.to_string()
        );
        let response = {
            let data = wasm.0.into_vec().await?;
            self.component_service
                .update(&component_id.0, data, &auth)
                .instrument(record.span.clone())
                .await
                .map_err(|e| e.into())
                .map(Json)
        };
        record.result(response)
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
        let record = recorded_http_api_request!(
            "create_component",
            component_name = payload.query.0.component_name.to_string(),
            project_id = payload.query.0.project_id.as_ref().map(|v| v.to_string()),
        );
        let response = {
            let data = payload.component.into_vec().await?;
            let component_name = payload.query.0.component_name;
            let project_id = payload.query.0.project_id;
            self.component_service
                .create(project_id, &component_name, data, &auth)
                .instrument(record.span.clone())
                .await
                .map_err(|e| e.into())
                .map(Json)
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
                Binary(Body::from_bytes_stream(bytes.map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
                })))
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
    ) -> Result<Json<crate::model::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
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
                .get_by_version(&versioned_component_id, &auth)
                .instrument(record.span.clone())
                .await
                .map_err(|e| e.into())
                .and_then(|response| match response {
                    Some(component) => Ok(Json(component)),
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
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::Component>> {
        let auth = CloudAuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "get_latest_component_metadata",
            component_id = component_id.0.to_string()
        );

        let response = self
            .component_service
            .get_latest_version(&component_id.0, &auth)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .and_then(|response| match response {
                Some(component) => Ok(Json(component)),
                None => Err(ComponentError::NotFound(Json(ErrorBody {
                    error: "Component not found".to_string(),
                }))),
            });

        record.result(response)
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
        let record = recorded_http_api_request!(
            "get_components",
            component_name = component_name.0.as_ref().map(|v| v.0.clone()),
            project_id = project_id.0.as_ref().map(|v| v.to_string()),
        );

        let response = self
            .component_service
            .find_by_project_and_name(project_id.0, component_name.0, &auth)
            .instrument(record.span.clone())
            .await
            .map_err(|e| e.into())
            .map(Json);

        record.result(response)
    }
}
