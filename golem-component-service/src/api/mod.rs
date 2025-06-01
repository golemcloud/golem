use crate::service::Services;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::SafeDisplay;
use golem_component_service_base::service::component::ComponentError as BaseComponentError;
use golem_component_service_base::service::plugin::PluginError;
use poem::error::ReadBodyError;
use poem::Route;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, OpenApiService, Tags};

pub mod component;
pub mod dto;
pub mod healthcheck;
pub mod plugin;

#[derive(Tags)]
enum ApiTags {
    Component,
    HealthCheck,
    Plugin,
}

pub fn combined_routes(services: &Services) -> Route {
    let api_service = make_open_api_service(services);

    let ui = api_service.swagger_ui();
    let spec = api_service.spec_endpoint_yaml();

    Route::new()
        .nest("/", api_service)
        .nest("/docs", ui)
        .nest("/specs", spec)
}

type ApiServices = (
    component::ComponentApi,
    healthcheck::HealthcheckApi,
    plugin::PluginApi,
);

pub fn make_open_api_service(services: &Services) -> OpenApiService<ApiServices, ()> {
    OpenApiService::new(
        (
            component::ComponentApi::new(
                services.component_service.clone(),
                services.api_mapper.clone(),
            ),
            healthcheck::HealthcheckApi,
            plugin::PluginApi::new(services.plugin_service.clone()),
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
            PluginError::BlobStorageError(_) => ComponentError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            PluginError::InvalidOplogProcessorPlugin => {
                ComponentError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
        }
    }
}

type Result<T> = std::result::Result<T, ComponentError>;

impl From<crate::service::CloudComponentError> for ComponentError {
    fn from(value: crate::service::CloudComponentError) -> Self {
        match value {
            crate::service::CloudComponentError::Unauthorized(_) => {
                ComponentError::Unauthorized(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            crate::service::CloudComponentError::BaseComponentError(
                BaseComponentError::ComponentProcessingError(_),
            ) => ComponentError::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
            crate::service::CloudComponentError::BaseComponentError(
                BaseComponentError::UnknownComponentId(_),
            )
            | crate::service::CloudComponentError::BaseComponentError(
                BaseComponentError::UnknownVersionedComponentId(_),
            )
            | crate::service::CloudComponentError::UnknownProject(_)
            | crate::service::CloudComponentError::BasePluginError(
                PluginError::ComponentNotFound { .. },
            ) => ComponentError::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            crate::service::CloudComponentError::LimitExceeded(_) => {
                ComponentError::LimitExceeded(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            crate::service::CloudComponentError::BaseComponentError(
                BaseComponentError::AlreadyExists(_),
            ) => ComponentError::AlreadyExists(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            crate::service::CloudComponentError::InternalAuthServiceError(_)
            | crate::service::CloudComponentError::BaseComponentError(_)
            | crate::service::CloudComponentError::BasePluginError(_)
            | crate::service::CloudComponentError::InternalLimitError(_)
            | crate::service::CloudComponentError::InternalProjectError(_) => {
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
