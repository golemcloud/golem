use futures_util::TryStreamExt;
use std::sync::Arc;

use cloud_common::auth::GolemSecurityScheme;
use golem_common::model::ProjectId;
use golem_common::model::TemplateId;
use poem::error::ReadBodyError;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::*;

use crate::api::ApiTags;
use crate::model::*;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::template::{TemplateError as TemplateServiceError, TemplateService};
use golem_service_base::model::*;

#[derive(ApiResponse)]
pub enum TemplateError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Maximum number of templates exceeded
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    /// Template not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    /// Template already exists
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

#[derive(Multipart)]
pub struct UploadPayload {
    query: JsonField<TemplateQuery>,
    template: Upload,
}

type Result<T> = std::result::Result<T, TemplateError>;

impl From<AuthServiceError> for TemplateError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => {
                TemplateError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Unexpected(error) => {
                TemplateError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<TemplateServiceError> for TemplateError {
    fn from(value: TemplateServiceError) -> Self {
        match value {
            TemplateServiceError::Unauthorized(error) => {
                TemplateError::Unauthorized(Json(ErrorBody { error }))
            }
            TemplateServiceError::TemplateProcessing(error) => {
                TemplateError::BadRequest(Json(ErrorsBody {
                    errors: vec![error.to_string()],
                }))
            }
            TemplateServiceError::UnknownTemplateId(_)
            | TemplateServiceError::UnknownVersionedTemplateId(_)
            | TemplateServiceError::UnknownProjectId(_) => {
                TemplateError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_string()],
                }))
            }
            TemplateServiceError::LimitExceeded(error) => {
                TemplateError::LimitExceeded(Json(ErrorBody { error }))
            }
            TemplateServiceError::AlreadyExists(_) => {
                TemplateError::AlreadyExists(Json(ErrorBody {
                    error: value.to_string(),
                }))
            }
            TemplateServiceError::Internal(error) => {
                TemplateError::InternalError(Json(ErrorBody {
                    error: error.to_string(),
                }))
            }
        }
    }
}

impl From<ReadBodyError> for TemplateError {
    fn from(value: ReadBodyError) -> Self {
        TemplateError::InternalError(Json(ErrorBody {
            error: value.to_string(),
        }))
    }
}

impl From<std::io::Error> for TemplateError {
    fn from(value: std::io::Error) -> Self {
        TemplateError::InternalError(Json(ErrorBody {
            error: value.to_string(),
        }))
    }
}

pub struct TemplateApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub template_service: Arc<dyn TemplateService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/templates", tag = ApiTags::Template)]
impl TemplateApi {
    /// Get the metadata for all template versions
    ///
    /// Each template can have multiple versions. Every time a new WASM is uploaded for a given template id, that creates a new version.
    /// This endpoint returns a list of all versions for the template id provided as part of the URL. Each element of the response describes a single version of a template, but does not contain the binary (WASM) itself:
    ///
    /// - `versionedTemplateId` associates a specific version with the template id
    /// - `userTemplateId` and protectedTemplateId are implementation details, not used elsewhere on the public API
    /// - `templateName` is the human-readable name of the template
    /// - `templateSize` is the WASM binary's size in bytes
    /// - `metadata` contains information extracted from the WASM itself
    /// - `metadata.exports` is a list of exported functions, including their parameter's and return value's types
    /// - `metadata.producers` is a list of producer information added by tooling, each consisting of a list of fields associating one or more values to a given key. This contains information about what compilers and other WASM related tools were used to construct the Golem template.
    #[oai(path = "/:template_id", method = "get")]
    async fn get_template_by_id(
        &self,
        template_id: Path<TemplateId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<crate::model::Template>>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let response = self.template_service.get(&template_id.0, &auth).await?;
        Ok(Json(response))
    }

    /// Update a template
    #[oai(path = "/:template_id/upload", method = "put")]
    async fn update_template(
        &self,
        template_id: Path<TemplateId>,
        wasm: Binary<Body>,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::Template>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let data = wasm.0.into_vec().await?;
        let response = self
            .template_service
            .update(&template_id.0, data, &auth)
            .await?;
        Ok(Json(response))
    }

    /// Upload a new template
    ///
    /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    #[oai(path = "/", method = "post")]
    async fn upload_template(
        &self,
        payload: UploadPayload,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::Template>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let data = payload.template.into_vec().await?;
        let template_name = payload.query.0.template_name;
        let project_id = payload.query.0.project_id;
        let response = self
            .template_service
            .create(project_id, &template_name, data, &auth)
            .await?;
        Ok(Json(response))
    }

    /// Download a template
    ///
    /// Downloads a specific version of the template's WASM.
    #[oai(path = "/:template_id/download", method = "get")]
    async fn download_template(
        &self,
        template_id: Path<TemplateId>,
        version: Query<Option<u64>>,
        token: GolemSecurityScheme,
    ) -> Result<Binary<Body>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let bytes = self
            .template_service
            .download_stream(&template_id.0, version.0, &auth)
            .await?;
        Ok(Binary(Body::from_bytes_stream(bytes.map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        }))))
    }

    /// Get the latest version of a given template
    ///
    /// Gets the latest version of a template.
    #[oai(path = "/:template_id/latest", method = "get")]
    async fn get_latest_template(
        &self,
        template_id: Path<TemplateId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::Template>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let response = self
            .template_service
            .get_latest_version(&template_id.0, &auth)
            .await?;

        match response {
            Some(template) => Ok(Json(template)),
            None => Err(TemplateError::NotFound(Json(ErrorBody {
                error: "Template not found".to_string(),
            }))),
        }
    }

    /// Get all templates
    ///
    /// Gets all templates, optionally filtered by project and/or template name.
    #[oai(path = "/", method = "get")]
    async fn get_all_templates(
        &self,
        /// Project ID to filter by
        #[oai(name = "project-id")]
        project_id: Query<Option<ProjectId>>,
        /// Template name to filter by
        #[oai(name = "template-name")]
        template_name: Query<Option<TemplateName>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<crate::model::Template>>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let response = self
            .template_service
            .find_by_project_and_name(project_id.0, template_name.0, &auth)
            .await?;

        Ok(Json(response))
    }
}
