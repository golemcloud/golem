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

use std::sync::Arc;

use crate::api::ApiTags;
use crate::service::template::{TemplateError as TemplateServiceError, TemplateService};
use golem_common::model::TemplateId;
use golem_service_base::model::*;
use poem::error::ReadBodyError;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::types::multipart::Upload;
use poem_openapi::*;


#[derive(ApiResponse)]
pub enum TemplateError {
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

#[derive(Multipart)]
pub struct UploadPayload {
    #[oai(rename = "name")]
    name: TemplateName,
    template: Upload,
}

type Result<T> = std::result::Result<T, TemplateError>;

impl From<TemplateServiceError> for TemplateError {
    fn from(value: TemplateServiceError) -> Self {
        match value {
            TemplateServiceError::Internal(error) => {
                TemplateError::InternalError(Json(ErrorBody { error }))
            }
            TemplateServiceError::TemplateProcessingError(error) => {
                TemplateError::BadRequest(Json(ErrorsBody {
                    errors: vec![error],
                }))
            }
            TemplateServiceError::UnknownTemplateId(_) => {
                TemplateError::NotFound(Json(ErrorBody {
                    error: "Template not found".to_string(),
                }))
            }
            TemplateServiceError::UnknownVersionedTemplateId(_) => {
                TemplateError::NotFound(Json(ErrorBody {
                    error: "Template not found".to_string(),
                }))
            }
            TemplateServiceError::IOError(error) => {
                TemplateError::InternalError(Json(ErrorBody { error }))
            }
            TemplateServiceError::AlreadyExists(_) => {
                TemplateError::AlreadyExists(Json(ErrorBody {
                    error: "Template already exists".to_string(),
                }))
            }
        }
    }
}

impl From<String> for TemplateError {
    fn from(value: String) -> Self {
        TemplateError::InternalError(Json(ErrorBody { error: value }))
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
    pub template_service: Arc<dyn TemplateService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/templates", tag = ApiTags::Template)]
impl TemplateApi {
    #[oai(path = "/:template_id", method = "get")]
    async fn get_template_by_id(
        &self,
        template_id: Path<TemplateId>,
    ) -> Result<Json<Vec<Template>>> {
        let response = self.template_service.get(&template_id.0).await?;
        Ok(Json(response))
    }

    #[oai(path = "/:template_id/upload", method = "put")]
    async fn update_template(
        &self,
        template_id: Path<TemplateId>,
        wasm: Binary<Body>,
    ) -> Result<Json<Template>> {
        let data = wasm.0.into_vec().await?;
        let response = self.template_service.update(&template_id.0, data).await?;
        Ok(Json(response))
    }

    #[oai(path = "/", method = "post")]
    async fn upload_template(&self, payload: UploadPayload) -> Result<Json<Template>> {
        let data = payload.template.into_vec().await?;
        let template_name = payload.name;
        let response = self.template_service.create(&template_name, data).await?;
        Ok(Json(response))
    }

    #[oai(path = "/:template_id/download", method = "get")]
    async fn download_template(
        &self,
        template_id: Path<TemplateId>,
        version: Query<Option<i32>>,
    ) -> Result<Binary<Body>> {
        let bytes = self
            .template_service
            .download(&template_id.0, version.0)
            .await?;
        Ok(Binary(Body::from(bytes)))
    }

    #[oai(path = "/:template_id/latest", method = "get")]
    async fn get_latest_version(&self, template_id: Path<TemplateId>) -> Result<Json<i32>> {
        let response = self
            .template_service
            .get_latest_version(&template_id.0)
            .await?;

        match response {
            Some(template) => Ok(Json(template.versioned_template_id.version)),
            None => Err(TemplateError::NotFound(Json(ErrorBody {
                error: "Template not found".to_string(),
            }))),
        }
    }

    #[oai(path = "/", method = "get")]
    async fn get_all_templates(
        &self,
        #[oai(name = "template-name")] template_name: Query<Option<TemplateName>>,
    ) -> Result<Json<Vec<Template>>> {
        let response = self.template_service.find_by_name(template_name.0).await?;

        Ok(Json(response))
    }

}
