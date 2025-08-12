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

use poem_openapi::ApiResponse;
use poem_openapi::payload::Json;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::metrics::api::TraceErrorKind;
use crate::services::component::ComponentError;
use golem_common::SafeDisplay;

#[derive(ApiResponse, Debug, Clone)]
pub enum ApiError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized request
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Forbidden Request
    #[oai(status = 403)]
    Forbidden(Json<ErrorBody>),
    /// Entity not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    #[oai(status = 409)]
    Conflict(Json<ErrorBody>),
    /// Limits of the plan exceeded
    #[oai(status = 422)]
    LimitExceeded(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl TraceErrorKind for ApiError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            ApiError::BadRequest(_) => "BadRequest",
            ApiError::NotFound(_) => "NotFound",
            ApiError::Unauthorized(_) => "Unauthorized",
            ApiError::InternalError(_) => "InternalError",
            ApiError::Conflict(_) => "Conflict",
            ApiError::Forbidden(_) => "Forbidden",
            ApiError::LimitExceeded(_) => "LimitExceeded"
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            ApiError::BadRequest(_) => true,
            ApiError::NotFound(_) => true,
            ApiError::Unauthorized(_) => true,
            ApiError::InternalError(_) => false,
            ApiError::Forbidden(_) => true,
            ApiError::Conflict(_) => true,
            ApiError::LimitExceeded(_) => true
        }
    }
}

impl From<ComponentError> for ApiError {
    fn from(value: ComponentError) -> Self {
        match value {
            ComponentError::Unauthorized(_) => {
                Self::Unauthorized(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }

            ComponentError::LimitExceeded { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }

            ComponentError::AlreadyExists(_) => {
                Self::Conflict(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }

            ComponentError::ComponentProcessingError(_)
            | ComponentError::InitialComponentFileNotFound { .. }
            | ComponentError::InvalidFilePath(_)
            | ComponentError::InvalidComponentName { .. }
            | ComponentError::InvalidOplogProcessorPlugin
            | ComponentError::InvalidPluginScope { .. }
            | ComponentError::ConcurrentUpdate { .. }
            | ComponentError::MalformedComponentArchive { .. }
            | ComponentError::PluginInstallationNotFound { .. } => {
                Self::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }

            ComponentError::UnknownComponentId(_)
            | ComponentError::UnknownVersionedComponentId(_)
            | ComponentError::PluginNotFound { .. } => {
                Self::NotFound(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }

            ComponentError::InternalError(_) => {
                Self::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
        }
    }
}
