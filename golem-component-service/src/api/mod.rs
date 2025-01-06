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

use crate::service::Services;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::SafeDisplay;
use golem_component_service_base::service::component::ComponentError as ComponentServiceError;
use golem_component_service_base::service::plugin::PluginError;
use golem_service_base::model::{ErrorBody, ErrorsBody};
use poem::endpoint::PrometheusExporter;
use poem::error::ReadBodyError;
use poem::Route;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, OpenApiService};
use prometheus::Registry;

pub mod component;
pub mod healthcheck;
pub mod plugin;

pub fn combined_routes(prometheus_registry: Registry, services: &Services) -> Route {
    let api_service = make_open_api_service(services);

    let ui = api_service.swagger_ui();
    let spec = api_service.spec_endpoint_yaml();
    let metrics = PrometheusExporter::new(prometheus_registry.clone());

    Route::new()
        .nest("/", api_service)
        .nest("/docs", ui)
        .nest("/specs", spec)
        .nest("/metrics", metrics)
}

pub type ApiServices = (
    component::ComponentApi,
    healthcheck::HealthcheckApi,
    plugin::PluginApi,
);

pub fn make_open_api_service(services: &Services) -> OpenApiService<ApiServices, ()> {
    OpenApiService::new(
        (
            component::ComponentApi {
                component_service: services.component_service.clone(),
                plugin_service: services.plugin_service.clone(),
            },
            healthcheck::HealthcheckApi,
            plugin::PluginApi {
                plugin_service: services.plugin_service.clone(),
            },
        ),
        "Golem API",
        "1.0",
    )
}

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

type Result<T> = std::result::Result<T, ComponentError>;

impl From<ComponentServiceError> for ComponentError {
    fn from(error: ComponentServiceError) -> Self {
        match error {
            ComponentServiceError::UnknownComponentId(_)
            | ComponentServiceError::UnknownVersionedComponentId(_) => {
                ComponentError::NotFound(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
            ComponentServiceError::AlreadyExists(_) => {
                ComponentError::AlreadyExists(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
            ComponentServiceError::ComponentProcessingError(error) => {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                }))
            }
            ComponentServiceError::InternalRepoError(_) => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
            ComponentServiceError::InternalConversionError { .. } => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
            ComponentServiceError::ComponentStoreError { .. } => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
            ComponentServiceError::ComponentConstraintConflictError(_) => {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                }))
            }
            ComponentServiceError::ComponentConstraintCreateError(_) => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
            ComponentServiceError::MalformedComponentArchiveError { .. } => {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                }))
            }
            ComponentServiceError::InitialComponentFileUploadError { .. } => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
            ComponentServiceError::InitialComponentFileNotFound { .. } => {
                ComponentError::NotFound(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
            ComponentServiceError::TransformationPluginNotFound { .. } => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
            ComponentServiceError::InternalPluginError(_) => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
            ComponentServiceError::TransformationFailed(_) => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: error.to_safe_string(),
                }))
            }
        }
    }
}

impl From<PluginError> for ComponentError {
    fn from(value: PluginError) -> Self {
        match value {
            PluginError::InternalRepoError(_) => ComponentError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            PluginError::InternalConversionError { .. } => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            PluginError::InternalComponentError(_) => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            PluginError::ComponentNotFound { .. } => ComponentError::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            PluginError::FailedToGetAvailableScopes { .. } => {
                ComponentError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            PluginError::PluginNotFound { .. } => ComponentError::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            PluginError::InvalidScope { .. } => ComponentError::Unauthorized(Json(ErrorBody {
                error: value.to_safe_string(),
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
