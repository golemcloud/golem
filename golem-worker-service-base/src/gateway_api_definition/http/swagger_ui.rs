use poem::Route;
use poem_openapi::{OpenApi, OpenApiService};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwaggerUiConfig {
    pub enabled: bool,
    pub title: Option<String>,
    pub version: Option<String>,
    pub server_url: Option<String>,
}

impl Default for SwaggerUiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            title: None,
            version: None,
            server_url: None,
        }
    }
}

/// Creates an OpenAPI service with optional Swagger UI
pub fn create_swagger_ui<T: OpenApi>(api: T, config: &SwaggerUiConfig) -> OpenApiService<T, ()> {
    let title = config.title.clone().unwrap_or_else(|| "API Documentation".to_string());
    let version = config.version.clone().unwrap_or_else(|| "1.0".to_string());
    let mut service = OpenApiService::new(api, title, version);
    if let Some(url) = &config.server_url {
        service = service.server(url);
    }
    service
}

/// Creates a route that includes both the API service and optionally the Swagger UI
pub fn create_api_route<T: OpenApi + Clone + 'static>(api: T, config: &SwaggerUiConfig) -> Route {
    let service = create_swagger_ui(api.clone(), config);
    let mut route = Route::new().nest("/", service);
    
    if config.enabled {
        let ui_service = create_swagger_ui(api, config);
        route = route.nest("/docs", ui_service.swagger_ui());
    }
    
    route
}
