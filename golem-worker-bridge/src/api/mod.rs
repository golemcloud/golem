pub mod api_definition_endpoints;
pub mod common;
pub mod custom_request_endpoint;
pub mod worker;
pub mod worker_connect;

use std::sync::Arc;

use crate::api::api_definition_endpoints::ApiDefinitionEndpoints;
use poem::Route;
use poem_openapi::OpenApiService;

use crate::register::RegisterApiDefinition;
use crate::worker_request_executor::WorkerRequestExecutor;

#[derive(Clone)]
pub struct ApiServices {
    pub definition_service: Arc<dyn RegisterApiDefinition + Sync + Send>,
    pub worker_request_executor: Arc<dyn WorkerRequestExecutor + Sync + Send>,
}

pub struct ManagementOpenApiService {
    pub route: Route,
    pub yaml: String,
}

pub fn management_open_api_service(services: ApiServices) -> ManagementOpenApiService {
    let api_service = OpenApiService::new(
        ApiDefinitionEndpoints::new(services.definition_service.clone()),
        "Golem Api Gateway Management API",
        "1.0",
    );

    let yaml = api_service.spec_yaml();

    let ui = api_service.swagger_ui();
    let spec = api_service.spec_endpoint_yaml();
    let route = Route::new()
        .nest("/", api_service)
        .nest("/v1/api/docs", ui)
        .nest("/v1/api/specs", spec);

    ManagementOpenApiService { route, yaml }
}

pub fn api_definition_routes(services: ApiServices) -> Route {
    management_open_api_service(services).route
}

pub fn custom_request_route(services: ApiServices) -> Route {
    let custom_request_executor = custom_request_endpoint::CustomRequestEndpoint::new(
        services.worker_request_executor,
        services.definition_service,
    );

    Route::new().nest("/", custom_request_executor)
}
