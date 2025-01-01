use poem::Route;
use poem_openapi::OpenApiService;

use super::healthcheck::HealthcheckApi;
use super::rib_endpoints::{RibApi, rib_routes};

pub fn create_api_router() -> Route {
    let api_service = OpenApiService::new((HealthcheckApi, RibApi), "Golem API", "1.0")
        .server("http://localhost:3000");
    
    let ui = api_service.swagger_ui();

    Route::new()
        // Mount RIB routes under /api/v1/rib
        .nest("/api/v1/rib", rib_routes())
        // Mount OpenAPI service
        .nest("/api", api_service)
        // Mount Swagger UI
        .nest("/swagger", ui)
} 