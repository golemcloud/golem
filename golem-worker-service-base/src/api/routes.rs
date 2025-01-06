use poem::Route;
use poem::Endpoint;
use poem::EndpointExt;
use std::sync::Arc;
use crate::service::gateway::api_definition::ApiDefinitionError;
use crate::gateway_middleware::{HttpCors, HttpMiddleware};
use poem::middleware::Middleware;
use poem::web::{Path, Json};
use poem::handler;
use serde::{Serialize, Deserialize};

use super::healthcheck::healthcheck_routes;
use super::rib_endpoints::RibApi;
use super::wit_types_api::WitTypesApi;
use poem_openapi::OpenApiService;
use crate::service::gateway::api_definition_validator::ApiDefinitionValidatorService;
use crate::service::component::ComponentService;
use crate::repo::api_definition::ApiDefinitionRepo;
use crate::repo::api_deployment::ApiDeploymentRepo;
use crate::service::gateway::security_scheme::SecuritySchemeService;
use golem_service_base::auth::DefaultNamespace;
use crate::gateway_api_definition::http::HttpApiDefinition;
use crate::gateway_api_definition::http::swagger_ui::SwaggerUiConfig;

#[derive(Debug, Serialize, Deserialize)]
struct DynamicRequest {
    message: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DynamicResponse {
    result: String,
}

#[handler]
async fn handle_test() -> poem::Result<String> {
    Ok("test".to_string())
}

#[handler]
async fn handle_dynamic(path: Path<String>, payload: Json<DynamicRequest>) -> poem::Result<Json<DynamicResponse>> {
    Ok(Json(DynamicResponse {
        result: format!("dynamic: {} - {}", path.0, payload.0.message),
    }))
}

/// Creates a consistent CORS middleware configuration used across the application
pub fn create_cors_middleware<E: Endpoint>() -> impl Middleware<E> {
    let cors = HttpCors::from_parameters(
        Some("http://localhost:3000".to_string()),
        Some("GET, POST, PUT, DELETE, OPTIONS, HEAD, PATCH".to_string()),
        Some("authorization, content-type, accept, request-origin, origin, x-requested-with, access-control-request-method, access-control-request-headers, access-control-allow-origin, user-agent, referer, host, connection, vary".to_string()),
        Some("content-type, content-length, authorization, accept, request-origin, origin, access-control-allow-origin, access-control-allow-methods, access-control-allow-headers, access-control-max-age, access-control-expose-headers, vary".to_string()),
        Some(true),
        Some(3600),
        Some(vec![
            "Origin".to_string(),
            "Access-Control-Request-Method".to_string(),
            "Access-Control-Request-Headers".to_string()
        ]),
    ).expect("Failed to create CORS configuration");

    HttpMiddleware::cors(cors)
}

pub async fn create_api_router<AuthCtx>(
    server_url: Option<String>,
    _component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
    _definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
    _deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send>,
    _security_scheme_service: Arc<dyn SecuritySchemeService<DefaultNamespace> + Sync + Send>,
    _api_definition_validator: Arc<dyn ApiDefinitionValidatorService<HttpApiDefinition> + Sync + Send>,
    swagger_config: Option<SwaggerUiConfig>,
) -> Result<impl Endpoint, ApiDefinitionError>
where
    AuthCtx: Send + Sync + Default + 'static,
{
    let server = server_url.unwrap_or_else(|| "http://localhost:3000".to_string());

    // Create API services
    let rib_api = OpenApiService::new(RibApi::new(), "RIB API", "1.0.0")
        .server(server.clone())
        .url_prefix("/api");
    
    let health_api = healthcheck_routes();
    
    let wit_api = OpenApiService::new(WitTypesApi, "WIT Types API", "1.0.0")
        .server(server)
        .url_prefix("/api/wit-types");

    // Create UI endpoints
    let rib_ui = rib_api.swagger_ui();
    let wit_ui = wit_api.swagger_ui();

    // Get spec endpoints before applying CORS
    let rib_spec = rib_api.spec_endpoint();
    let wit_spec = wit_api.spec_endpoint();

    // Create the base route tree
    let mut base_route = Route::new()
        .at("/api/openapi", rib_spec)
        .at("/api/wit-types/doc", wit_spec)
        .nest("/api/v1/swagger-ui", rib_ui)
        .nest("/api/wit-types/swagger-ui", wit_ui)
        .nest("/api", rib_api)
        .nest("/api/wit-types", wit_api)
        .nest("/api/v1", health_api);

    // If swagger config is provided, add dynamic routes
    if let Some(config) = swagger_config {
        if let Some(worker_binding) = config.worker_binding {
            // Add dynamic routes based on worker binding configuration
            let mount_path = worker_binding.mount_path.trim_start_matches('/');
            base_route = base_route
                .nest(
                    &format!("/{}", mount_path),
                    Route::new()
                        .at("/test", handle_test)
                        .at("/dynamic/:path", handle_dynamic)
                );
        }
    }

    // Apply middleware to the base route
    Ok(base_route
        .with(poem::middleware::AddData::new(()))
        .with(create_cors_middleware()))
} 