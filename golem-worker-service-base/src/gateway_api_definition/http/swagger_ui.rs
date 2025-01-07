use poem::Route;
use poem_openapi::{
    OpenApi, OpenApiService, SecurityScheme, OAuthScopes,
    auth::Bearer,
};
use serde::{Deserialize, Serialize};

/// Configuration for Swagger UI authentication via Worker Gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwaggerUiAuthConfig {
    pub enabled: bool,
    pub auth_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub scopes: Vec<String>,
}

impl Default for SwaggerUiAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auth_endpoint: None,
            token_endpoint: None,
            scopes: Vec::new(),
        }
    }
}

/// Configuration for binding Swagger UI to a worker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwaggerUiWorkerBinding {
    pub worker_name: String,
    pub component_id: String,
    pub component_version: Option<u32>,
    pub mount_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwaggerUiConfig {
    pub enabled: bool,
    pub title: Option<String>,
    pub version: Option<String>,
    pub server_url: Option<String>,
    pub auth: SwaggerUiAuthConfig,
    pub worker_binding: Option<SwaggerUiWorkerBinding>,
    /// OpenAPI extensions for Golem-specific bindings
    #[serde(default)]
    pub golem_extensions: std::collections::HashMap<String, serde_json::Value>,
}

impl Default for SwaggerUiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            title: None,
            version: None,
            server_url: None,
            auth: SwaggerUiAuthConfig::default(),
            worker_binding: None,
            golem_extensions: std::collections::HashMap::new(),
        }
    }
}

#[allow(dead_code)]
#[derive(OAuthScopes)]
enum ApiScopes {
    /// Read access to the API
    Read,
    /// Write access to the API
    Write,
    /// Admin access to the API
    Admin,
    /// OpenID Connect authentication
    OpenId,
    /// Access to email information
    Email,
    /// Access to profile information
    Profile,
}

#[allow(dead_code)]
#[derive(SecurityScheme)]
#[oai(
    ty = "oauth2",
    flows(
        implicit(
            authorization_url = "auth_url",
            scopes = "ApiScopes"
        ),
        authorization_code(
            authorization_url = "auth_url",
            token_url = "token_url",
            scopes = "ApiScopes"
        )
    )
)]
struct OAuth2(Bearer);

/// Creates an OpenAPI service with optional Swagger UI and worker binding support
pub fn create_swagger_ui<T: OpenApi>(api: T, config: &SwaggerUiConfig) -> OpenApiService<T, ()> {
    let title = config.title.clone().unwrap_or_else(|| "API Documentation".to_string());
    let version = config.version.clone().unwrap_or_else(|| "1.0".to_string());
    let mut service = OpenApiService::new(api, title, version);

    // Configure server URL
    if let Some(url) = &config.server_url {
        service = service.server(url);
    }

    // Add Golem-specific metadata through description
    let mut description = String::new();
    if let Some(binding) = &config.worker_binding {
        description.push_str(&format!(
            "\nx-golem-worker-binding:\n  workerName: {}\n  componentId: {}\n  componentVersion: {}\n  mountPath: {}\n",
            binding.worker_name,
            binding.component_id,
            binding.component_version.unwrap_or(0),
            binding.mount_path,
        ));
    }

    // Add OAuth2 configuration if enabled
    if config.auth.enabled {
        if let (Some(auth_url), Some(token_url)) = (&config.auth.auth_endpoint, &config.auth.token_endpoint) {
            description.push_str(&format!(
                "\nx-golem-auth:\n  type: oauth2\n  authUrl: {}\n  tokenUrl: {}\n  scopes:\n",
                auth_url, token_url
            ));
            for scope in &config.auth.scopes {
                description.push_str(&format!("    - {}\n", scope));
            }
        }
    }

    // Add other Golem extensions
    for (key, value) in &config.golem_extensions {
        description.push_str(&format!("\nx-golem-{}:\n{}\n", key, serde_json::to_string_pretty(value).unwrap()));
    }

    if !description.is_empty() {
        service = service.description(description);
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
