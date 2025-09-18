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

pub mod agent_types;
pub mod common;
pub mod component;
pub mod dto;
pub mod plugin;

use crate::bootstrap::Services;
use crate::error::ComponentError as DomainComponentError;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::SafeDisplay;
use golem_service_base::api::HealthcheckApi;
use poem::error::ReadBodyError;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, OpenApiService};

pub type Apis = (
    HealthcheckApi,
    component::ComponentApi,
    plugin::PluginApi,
    agent_types::AgentTypesApi,
);

pub fn make_open_api_service(services: &Services) -> OpenApiService<Apis, ()> {
    OpenApiService::new(
        (
            HealthcheckApi,
            component::ComponentApi::new(
                services.component_service.clone(),
                services.api_mapper.clone(),
            ),
            plugin::PluginApi::new(services.plugin_service.clone()),
            agent_types::AgentTypesApi::new(services.agent_types_service.clone()),
        ),
        "Golem API",
        "1.0",
    )
}

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

type Result<T> = std::result::Result<T, ComponentError>;

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

    fn is_expected(&self) -> bool {
        match &self {
            ComponentError::BadRequest(_) => true,
            ComponentError::NotFound(_) => true,
            ComponentError::AlreadyExists(_) => true,
            ComponentError::LimitExceeded(_) => true,
            ComponentError::Unauthorized(_) => true,
            ComponentError::InternalError(_) => false,
        }
    }
}

impl From<DomainComponentError> for ComponentError {
    fn from(value: DomainComponentError) -> Self {
        match value {
            DomainComponentError::Unauthorized(_) => {
                ComponentError::Unauthorized(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }

            DomainComponentError::LimitExceeded(_) => {
                ComponentError::LimitExceeded(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }

            DomainComponentError::AlreadyExists(_) => {
                ComponentError::AlreadyExists(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }

            DomainComponentError::ComponentProcessingError(_)
            | DomainComponentError::InitialComponentFileNotFound { .. }
            | DomainComponentError::InvalidFilePath(_)
            | DomainComponentError::InvalidComponentName { .. }
            | DomainComponentError::MalformedComponentArchiveError { .. }
            | DomainComponentError::ComponentConstraintConflictError(_)
            | DomainComponentError::InvalidOplogProcessorPlugin
            | DomainComponentError::InvalidPluginScope { .. }
            | DomainComponentError::ConcurrentUpdate { .. }
            | DomainComponentError::PluginInstallationNotFound { .. } => {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }

            DomainComponentError::UnknownComponentId(_)
            | DomainComponentError::UnknownVersionedComponentId(_)
            | DomainComponentError::UnknownProject(_)
            | DomainComponentError::PluginNotFound { .. } => {
                ComponentError::NotFound(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }

            DomainComponentError::InternalAuthServiceError(_)
            | DomainComponentError::InternalLimitError(_)
            | DomainComponentError::InternalProjectError(_)
            | DomainComponentError::InternalRepoError(_)
            | DomainComponentError::InternalConversionError { .. }
            | DomainComponentError::ComponentStoreError { .. }
            | DomainComponentError::ComponentConstraintCreateError(_)
            | DomainComponentError::InitialComponentFileUploadError { .. }
            | DomainComponentError::TransformationFailed(_)
            | DomainComponentError::PluginApplicationFailed(_)
            | DomainComponentError::FailedToDownloadFile
            | DomainComponentError::BlobStorageError(_) => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
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
